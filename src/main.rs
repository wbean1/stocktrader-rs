use std::collections::HashMap;
use std::env;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use clap::{Parser, Subcommand};
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use rayon::prelude::*;
use time::OffsetDateTime;
use time::Time;
use tokio_test;
use yahoo_finance_api as yahoo;
use yahoo_finance_api::Quote;
use yahoo_finance_api::{Dividend, Split};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Quote {
        #[arg(short, long)]
        ticker: String,

        #[arg(short, long)]
        filter_pct: Option<f32>,
    },
    Simulate {
        #[arg(short, long)]
        tickers: Vec<String>,
    },
    Alert {
        #[arg(short, long)]
        tickers: Vec<String>,

        #[arg(short, long)]
        buy_when: f32,

        #[arg(short, long)]
        sell_when: f32,
    },
}

#[derive(Clone)]
struct TickeredQuote {
    ticker: String,
    quote: Quote,
}

#[derive(Clone, Debug, Default)]
struct CorporateActions {
    splits: HashMap<String, Vec<Split>>,
    dividends: HashMap<String, Vec<Dividend>>,
}

#[derive(Clone, Copy, Debug)]
struct SimulationResult {
    buy_when: f32,
    sell_when: f32,
    buy_increment: f32,
    net_worth: f32,
}

const STARTING_CASH: f32 = 100000.0;

fn main() {
    let args = Args::parse();
    let (start, end) = default_backtest_window_10y_ending_last_weekday_utc();
    match &args.command {
        Some(Commands::Quote { ticker, filter_pct }) => {
            let quotes = match get_quote_history(ticker, start, end) {
                Ok(quotes) => quotes,
                Err(err) => {
                    eprintln!("failed to fetch quote history for {ticker}: {err}");
                    return;
                }
            };
            println!("got quotes for {} days", quotes.len());
            println!("{0: <11} | {1: <10} | {2: <9}", "date", "close", "pct");
            let mut previous_close: Option<f32> = None;
            for q in quotes.iter() {
                let date = Utc.timestamp_opt(q.timestamp as i64, 0).unwrap();
                let date_str = date.format("%Y-%m-%d").to_string();
                match previous_close {
                    Some(prev_close) => {
                        let pct = pct_change(prev_close, q.close as f32);
                        match filter_pct {
                            Some(filter) => {
                                if pct.abs() >= *filter {
                                    println!(
                                        "{0: <11} | {1:7.2} | {2:6.2}",
                                        date_str, q.close, pct
                                    );
                                }
                            }
                            None => {
                                println!("{0: <11} | {1:7.2} | {2:6.2}", date_str, q.close, pct)
                            }
                        }
                    }
                    None => println!("{0: <11} | {1:7.2} | {2:>6}", date_str, q.close, "N/A"),
                }
                previous_close = Some(q.close as f32);
            }
        }
        Some(Commands::Simulate { tickers }) => {
            if tickers.is_empty() {
                eprintln!("simulate requires at least one ticker");
                return;
            }

            let (quotes, actions) = match get_quotes_and_actions_for_tickers(tickers, start, end) {
                Ok(result) => result,
                Err(err) => {
                    eprintln!("failed to load historical quotes: {err}");
                    return;
                }
            };
            let closes = match get_latest_closes_for_tickers(tickers) {
                Ok(closes) => closes,
                Err(err) => {
                    eprintln!("failed to load latest closes: {err}");
                    return;
                }
            };

            let search_space = build_parameter_grid();
            let best = search_space
                .par_iter()
                .map(|&(buy_when, sell_when, buy_increment)| {
                    let (holdings, cash_spent, cash_made) =
                        simulate(&quotes, &actions, buy_increment, buy_when, sell_when);
                    let net_worth =
                        calc_net_worth_with_closes(cash_made - cash_spent, holdings, &closes);
                    SimulationResult {
                        buy_when,
                        sell_when,
                        buy_increment,
                        net_worth,
                    }
                })
                .max_by(|left, right| left.net_worth.partial_cmp(&right.net_worth).unwrap());

            if let Some(best) = best {
                println!(
                    "Best Net-Worth: ${:.2} ({:.2}% total return), buy_when: -{:.1}%, sell_when: {:.1}%, buy_increment: ${:.2}",
                    best.net_worth,
                    total_return_pct(best.net_worth),
                    best.buy_when,
                    best.sell_when,
                    best.buy_increment
                );
            }

            let buy_and_hold = calc_buy_and_hold_strategy(tickers, &quotes, &actions, &closes);
            println!(
                "Buy & Hold: ${:.2} ({:.2}% total return)",
                buy_and_hold,
                total_return_pct(buy_and_hold)
            );
        }
        Some(Commands::Alert {
            tickers,
            buy_when,
            sell_when,
        }) => {
            for ticker in tickers.iter() {
                match get_latest_change(ticker) {
                    Ok(pct_change) => {
                        if -1.0 * pct_change >= *buy_when || pct_change >= *sell_when {
                            if let Err(err) = send_alert_email(ticker, pct_change) {
                                eprintln!("failed to send alert for {ticker}: {err}");
                            }
                        }
                    }
                    Err(err) => eprintln!("failed to compute latest change for {ticker}: {err}"),
                }
            }
        }
        None => {}
    }
}

fn default_backtest_window_10y_ending_last_weekday_utc() -> (OffsetDateTime, OffsetDateTime) {
    let mut end_date = OffsetDateTime::now_utc().date();
    while end_date.weekday().number_from_monday() > 5 {
        end_date = end_date.previous_day().unwrap();
    }

    let end = end_date
        .with_time(Time::from_hms_nano(23, 59, 59, 990_000_000).unwrap())
        .assume_utc();

    let end_year = end_date.year();
    let target_year = end_year - 10;
    let start_date = end_date
        .replace_year(target_year)
        .or_else(|_| {
            // Handles leap-day edge case (e.g. Feb 29 -> Feb 28).
            end_date
                .replace_day(28)
                .ok()
                .and_then(|d| d.replace_year(target_year).ok())
                .ok_or(())
        })
        .unwrap();

    let start = start_date
        .with_time(Time::from_hms(0, 0, 0).unwrap())
        .assume_utc();

    (start, end)
}

fn pct_change(previous_close: f32, current_close: f32) -> f32 {
    ((current_close - previous_close) * 100.0) / previous_close
}

fn total_return_pct(net_worth: f32) -> f32 {
    ((net_worth - STARTING_CASH) / STARTING_CASH) * 100.0
}

fn build_parameter_grid() -> Vec<(f32, f32, f32)> {
    const BUY_INCREMENT_MIN: u32 = 1_000;
    const BUY_INCREMENT_MAX: u32 = 100_000;
    const BUY_INCREMENT_STEPS: usize = 25;
    const BUY_INCREMENT_ROUND_TO: u32 = 1_000;

    const THRESHOLD_MIN_PCT: f32 = 1.0;
    const THRESHOLD_MAX_PCT: f32 = 7.9;
    const THRESHOLD_STEP_PCT: f32 = 0.1;

    let buy_increment_delta = (BUY_INCREMENT_MAX - BUY_INCREMENT_MIN) as f32
        / (BUY_INCREMENT_STEPS.saturating_sub(1) as f32);
    let mut buy_increments: Vec<u32> = (0..BUY_INCREMENT_STEPS)
        .map(|i| BUY_INCREMENT_MIN as f32 + (i as f32) * buy_increment_delta)
        .map(|x| ((x / BUY_INCREMENT_ROUND_TO as f32).round() as u32) * BUY_INCREMENT_ROUND_TO)
        .map(|x| x.clamp(BUY_INCREMENT_MIN, BUY_INCREMENT_MAX))
        .collect();
    // Guarantee endpoints are present without increasing sampling count.
    if let Some(first) = buy_increments.first_mut() {
        *first = BUY_INCREMENT_MIN;
    }
    if let Some(last) = buy_increments.last_mut() {
        *last = BUY_INCREMENT_MAX;
    }

    buy_increments.sort_unstable();
    buy_increments.dedup();

    let thresholds = inclusive_pct_axis(THRESHOLD_MIN_PCT, THRESHOLD_MAX_PCT, THRESHOLD_STEP_PCT);
    assert_eq!(thresholds.len(), 70);
    assert!((thresholds[0] - THRESHOLD_MIN_PCT).abs() < 1e-3);
    assert!((thresholds[thresholds.len() - 1] - THRESHOLD_MAX_PCT).abs() < 1e-3);

    let grid_len = thresholds.len() * thresholds.len() * buy_increments.len();
    let mut search_space = Vec::with_capacity(grid_len);
    for &buy_when in thresholds.iter() {
        for &sell_when in thresholds.iter() {
            for &buy_increment in buy_increments.iter() {
                search_space.push((buy_when, sell_when, buy_increment as f32));
            }
        }
    }

    search_space
}

/// Inclusive axis from `min` to `max` advancing by `step` (e.g. 1.0, 1.1, …, 7.9).
fn inclusive_pct_axis(min: f32, max: f32, step: f32) -> Vec<f32> {
    assert!(step > 0.0);
    let span = max - min;
    let count = (span / step).round() as usize + 1;
    (0..count).map(|i| min + i as f32 * step).collect()
}

fn calc_net_worth_with_closes(
    cash: f32,
    holdings: HashMap<String, f32>,
    closes: &HashMap<String, f32>,
) -> f32 {
    let mut net_worth = cash;
    for (h, q) in holdings.iter() {
        if let Some(close) = closes.get(h) {
            net_worth += *q * close;
        }
    }
    net_worth
}

fn get_quote_history(
    ticker: &str,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> Result<Vec<Quote>, String> {
    let provider = yahoo::YahooConnector::new().map_err(|err| err.to_string())?;
    let response = tokio_test::block_on(provider.get_quote_history(ticker, start, end))
        .map_err(|err| err.to_string())?;
    response.quotes().map_err(|err| err.to_string())
}

fn get_quote_history_with_actions(
    ticker: &str,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> Result<(Vec<Quote>, Vec<Split>, Vec<Dividend>), String> {
    let provider = yahoo::YahooConnector::new().map_err(|err| err.to_string())?;
    let response = tokio_test::block_on(provider.get_quote_history(ticker, start, end))
        .map_err(|err| err.to_string())?;

    let quotes = response.quotes().map_err(|err| err.to_string())?;
    let splits = response.splits().map_err(|err| err.to_string())?;
    let dividends = response.dividends().map_err(|err| err.to_string())?;

    Ok((quotes, splits, dividends))
}

fn get_latest_close(ticker: &str) -> Result<f32, String> {
    let provider = yahoo::YahooConnector::new().map_err(|err| err.to_string())?;
    let response = tokio_test::block_on(provider.get_latest_quotes(ticker, "1d"))
        .map_err(|err| err.to_string())?;
    let quote = response.last_quote().map_err(|err| err.to_string())?;
    Ok(quote.close as f32)
}

fn get_latest_closes_for_tickers(tickers: &Vec<String>) -> Result<HashMap<String, f32>, String> {
    let mut closes: HashMap<String, f32> = HashMap::new();
    for t in tickers.iter() {
        let close = get_latest_close(t)?;
        closes.insert(t.to_string(), close);
    }
    Ok(closes)
}

fn get_quotes_and_actions_for_tickers(
    tickers: &Vec<String>,
    start: OffsetDateTime,
    end: OffsetDateTime,
) -> Result<(Vec<TickeredQuote>, CorporateActions), String> {
    let mut all_quotes: Vec<TickeredQuote> = Vec::new();
    let mut actions = CorporateActions::default();

    for t in tickers.iter() {
        let (quotes, mut splits, mut dividends) = get_quote_history_with_actions(t, start, end)?;
        splits.sort_unstable_by_key(|s| s.date);
        dividends.sort_unstable_by_key(|d| d.date);
        actions.splits.insert(t.to_string(), splits);
        actions.dividends.insert(t.to_string(), dividends);

        for q in quotes.iter() {
            all_quotes.push(TickeredQuote {
                ticker: t.to_string(),
                quote: q.clone(),
            })
        }
    }
    all_quotes.sort_by_key(|k| k.quote.timestamp);
    Ok((all_quotes, actions))
}

fn calc_buy_and_hold_strategy(
    tickers: &Vec<String>,
    quotes: &Vec<TickeredQuote>,
    actions: &CorporateActions,
    closes: &HashMap<String, f32>,
) -> f32 {
    let amount_per_ticker = STARTING_CASH / tickers.len() as f32;
    let mut holdings: HashMap<String, f32> = HashMap::new();
    let mut cash: f32 = 0.0;
    let mut purchases: HashMap<String, HashMap<DateTime<Utc>, f32>> = HashMap::new();
    for t in tickers.iter() {
        for q in quotes.iter() {
            if q.ticker == *t {
                let amount_to_buy = amount_per_ticker / q.quote.close as f32;
                holdings.insert(t.to_string(), amount_to_buy);
                purchases.insert(
                    t.to_string(),
                    HashMap::from([(
                        Utc.timestamp_opt(q.quote.timestamp as i64, 0).unwrap(),
                        amount_to_buy,
                    )]),
                );
                break;
            }
        }
    }

    // Apply corporate actions across the full timeline so buy-and-hold is comparable.
    let mut split_idx: HashMap<String, usize> = HashMap::new();
    let mut div_idx: HashMap<String, usize> = HashMap::new();
    for q in quotes.iter() {
        apply_corporate_actions_on_or_before_quote(
            &q.ticker,
            q.quote.timestamp,
            &mut holdings,
            &mut purchases,
            &mut cash,
            actions,
            &mut split_idx,
            &mut div_idx,
            None,
        );
    }

    calc_net_worth_with_closes(cash, holdings, closes)
}

fn simulate(
    quotes: &Vec<TickeredQuote>,
    actions: &CorporateActions,
    buy_increment: f32,
    buy_when: f32,
    sell_when: f32,
) -> (HashMap<String, f32>, f32, f32) {
    let mut holdings: HashMap<String, f32> = HashMap::new();
    let mut purchases: HashMap<String, HashMap<DateTime<Utc>, f32>> = HashMap::new();
    let mut cash_spent: f32 = 0.0;
    let mut cash_made: f32 = 0.0;
    let mut prev_closes: HashMap<String, f32> = HashMap::new();
    let mut split_idx: HashMap<String, usize> = HashMap::new();
    let mut div_idx: HashMap<String, usize> = HashMap::new();
    let cash_per_purchase: f32 = buy_increment;
    let cash_limit: f32 = STARTING_CASH;
    for q in quotes.iter() {
        let date = Utc.timestamp_opt(q.quote.timestamp as i64, 0).unwrap();

        apply_corporate_actions_on_or_before_quote(
            &q.ticker,
            q.quote.timestamp,
            &mut holdings,
            &mut purchases,
            &mut cash_made,
            actions,
            &mut split_idx,
            &mut div_idx,
            Some(&mut prev_closes),
        );

        let prev_close = match prev_closes.get(&q.ticker) {
            Some(a) => a,
            None => {
                prev_closes.insert(q.ticker.clone(), q.quote.close as f32);
                continue;
            }
        };
        let pct = pct_change(*prev_close, q.quote.close as f32);
        if pct <= -1.0 * buy_when && cash_spent + cash_per_purchase - cash_made < cash_limit {
            // buy low
            let amount_to_buy = cash_per_purchase / (q.quote.close as f32);
            cash_spent += cash_per_purchase;
            match holdings.get(&q.ticker) {
                Some(amount_owned) => {
                    holdings.insert(q.ticker.clone(), amount_owned + amount_to_buy)
                }
                None => holdings.insert(q.ticker.clone(), amount_to_buy),
            };
            let mut purchase_map: HashMap<DateTime<Utc>, f32> =
                purchases.get(&q.ticker).unwrap_or(&HashMap::new()).clone();
            purchase_map.insert(date, amount_to_buy);
            purchases.insert(q.ticker.clone(), purchase_map.clone());
        };
        if pct >= sell_when {
            // sell high
            let mut to_remove: Vec<DateTime<Utc>> = Vec::new();
            let mut purchase_map: HashMap<DateTime<Utc>, f32> =
                purchases.get(&q.ticker).unwrap_or(&HashMap::new()).clone();
            for (k, v) in purchase_map.iter() {
                if *k <= date - Duration::from_secs(60 * 60 * 24 * 365) {
                    // now - 1yr
                    // we can sell
                    cash_made += *v * q.quote.close as f32;
                    holdings.insert(q.ticker.clone(), holdings.get(&q.ticker).unwrap() - *v);
                    to_remove.push(*k);
                }
            }
            for k in to_remove.iter() {
                purchase_map.remove(k);
            }
            purchases.insert(q.ticker.clone(), purchase_map);
        }
        prev_closes.insert(q.ticker.clone(), q.quote.close as f32);
    }
    (holdings, cash_spent, cash_made)
}

fn apply_corporate_actions_on_or_before_quote(
    ticker: &str,
    quote_ts: u64,
    holdings: &mut HashMap<String, f32>,
    purchases: &mut HashMap<String, HashMap<DateTime<Utc>, f32>>,
    cash: &mut f32,
    actions: &CorporateActions,
    split_idx: &mut HashMap<String, usize>,
    div_idx: &mut HashMap<String, usize>,
    mut prev_closes: Option<&mut HashMap<String, f32>>,
) {
    // Apply any splits up to (and including) this quote timestamp.
    let splits = actions
        .splits
        .get(ticker)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let s_idx = split_idx.entry(ticker.to_string()).or_insert(0);
    while *s_idx < splits.len() && splits[*s_idx].date <= quote_ts {
        let split = &splits[*s_idx];

        // Yahoo's split event is expressed as a ratio numerator:denominator.
        // For a typical 5-for-1 split, the data comes through as numerator=5, denominator=1,
        // so the share multiplier should be numerator / denominator.
        let factor = (split.numerator / split.denominator) as f32;
        if factor.is_finite() && factor > 0.0 {
            if let Some(shares) = holdings.get_mut(ticker) {
                *shares *= factor;
            }

            if let Some(lots) = purchases.get_mut(ticker) {
                for (_k, v) in lots.iter_mut() {
                    *v *= factor;
                }
            }

            // Keep price continuity for pct-change logic: post-split prices are ~1/factor of pre-split.
            if let Some(prev_closes) = prev_closes.as_deref_mut() {
                if let Some(prev) = prev_closes.get_mut(ticker) {
                    *prev *= factor;
                }
            }
        }
        *s_idx += 1;
    }

    // Apply any dividends up to (and including) this quote timestamp.
    let dividends = actions
        .dividends
        .get(ticker)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let d_idx = div_idx.entry(ticker.to_string()).or_insert(0);
    while *d_idx < dividends.len() && dividends[*d_idx].date <= quote_ts {
        let div = &dividends[*d_idx];
        if let Some(shares) = holdings.get(ticker) {
            *cash += *shares * (div.amount as f32);
        }
        *d_idx += 1;
    }
}

fn get_latest_change(ticker: &String) -> Result<f32, String> {
    let start = time::OffsetDateTime::now_utc() - Duration::from_secs(60 * 60 * 24 * 7); // now - 1wk
    let end = time::OffsetDateTime::now_utc();
    let quotes = get_quote_history(ticker, start, end)?;
    if quotes.len() < 2 {
        return Err(format!("not enough quotes returned for {ticker}"));
    }

    let previous = &quotes[quotes.len() - 2];
    let current = &quotes[quotes.len() - 1];
    let pct_change = pct_change(previous.close as f32, current.close as f32);
    println!("[{}]: Got last day pct_change: {}", ticker, pct_change);
    Ok(pct_change)
}

fn send_alert_email(ticker: &String, pct_change: f32) -> Result<(), String> {
    let email = Message::builder()
        .from(
            env::var("EMAIL_ADDRESS")
                .map_err(|err| err.to_string())?
                .parse::<Mailbox>()
                .map_err(|err| err.to_string())?,
        )
        .to(env::var("EMAIL_ADDRESS")
            .map_err(|err| err.to_string())?
            .parse::<Mailbox>()
            .map_err(|err| err.to_string())?)
        .subject("Alert from stocktrader-rs")
        .body(String::from(format!(
            "Stock: {}, Change: {}",
            ticker, pct_change
        )))
        .map_err(|err| err.to_string())?;

    let creds = Credentials::new(
        env::var("EMAIL_ADDRESS").map_err(|err| err.to_string())?,
        env::var("SMTP_PASSWORD").map_err(|err| err.to_string())?,
    );

    // Open a remote connection to gmail
    let mailer = SmtpTransport::relay("smtp.gmail.com")
        .map_err(|err| err.to_string())?
        .credentials(creds)
        .build();

    // Send the email
    match mailer.send(&email) {
        Ok(_) => println!("Email sent successfully!"),
        Err(e) => return Err(format!("could not send email: {:?}", e)),
    }

    Ok(())
}
