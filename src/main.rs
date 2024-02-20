use std::collections::HashMap;
use std::env;
use std::time::Duration;
use std::sync::mpsc;
use std::thread;

use clap::{Parser, Subcommand};
use yahoo_finance_api as yahoo;
use yahoo_finance_api::Quote;
use chrono::{DateTime, TimeZone, Utc};
use time::macros::datetime;
use time::OffsetDateTime;
use tokio_test;
use lettre::transport::smtp::authentication::Credentials; 
use lettre::{Message, SmtpTransport, Transport}; 

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
    }
}

#[derive(Clone)]
struct TickeredQuote {
    ticker: String,
    quote: Quote,
}
const STARTING_CASH: f32 = 100000.0;
const START: OffsetDateTime = datetime!(2014-2-15 0:00:00.00 UTC);
const END: OffsetDateTime = datetime!(2024-2-14 23:59:59.99 UTC);

fn main() {
    let args = Args::parse();
    match &args.command {
        Some(Commands::Quote { ticker, filter_pct}) => {
            let provider = yahoo::YahooConnector::new();
            let response = tokio_test::block_on(provider.get_quote_history(ticker, START, END)).unwrap();
            let quotes = response.quotes().unwrap();
            println!("got quotes for {} days", quotes.len());
            println!("{0: <11} | {1: <10} | {2: <9}", "date", "close", "pct");
            let mut previous_close: f32 = 0.0;
            for q in quotes.iter() {
                let date = Utc.timestamp_opt(q.timestamp as i64, 0).unwrap();
                let date_str = date.format("%Y-%m-%d").to_string();
                let pct: f32 = (q.close as f32 - previous_close) * 100.0 / previous_close;
                match filter_pct {
                    Some(filter) => {
                        if pct.abs() >= *filter {
                            println!("{0: <11} | {1:7.2} | {2:6.2}", date_str, q.close, pct);
                        }
                    },
                    None => println!("{0: <11} | {1:7.2} | {2:6.2}", date_str, q.close, pct),
                }
                previous_close = q.close as f32;
            }
        },
        Some(Commands::Simulate { tickers }) => {
            let quotes = get_quotes_for_tickers(tickers);
            let closes = get_closes_for_tickers(tickers);
            let mut best_net_worth: f32 = 0.0;
            let (tx, rx) = mpsc::channel();

            for b in 10u8..50 {
                let buy_when = f32::from(b) * 0.1;
                for s in 10u8..80 {
                    let sell_when = f32::from(s) * 0.1;
                    for buy_increment in [1000.0, 3333.0, 5000.0, 10000.0, 20000.0, 33333.0, 50000.0, 100000.0 as f32] {
                        let tx = tx.clone();
                        let quotes = quotes.clone();
                        let closes = closes.clone();
                        thread::spawn(move || {
                            let (holdings, cash_spent, cash_made) = simulate(&quotes, buy_increment, buy_when, sell_when);
                            let net_worth = calc_net_worth_with_closes(cash_made - cash_spent, holdings, closes);
                            let _ = tx.send((net_worth, buy_when, sell_when, buy_increment));
                        });
                    }
                }
            }
            drop(tx);

            while let Ok((net_worth, buy_when, sell_when, buy_increment)) = rx.recv() {
                if net_worth > best_net_worth {
                    best_net_worth = net_worth;
                    println!("Hit best_net_worth: {:?}, buy_when: {:?}, sell_when: {:?}, buy_increment: {:?}", best_net_worth, buy_when, sell_when, buy_increment);
                }
            }
            println!("Buy & Hold: {:?}", calc_buy_and_hold_strategy(tickers))
        },
        Some(Commands::Alert { tickers, buy_when, sell_when }) => {
            for ticker in tickers.iter() {
                let pct_change = get_latest_change(ticker);
                if -1.0 * pct_change >= *buy_when ||
                   pct_change >= *sell_when {
                    send_alert_email(ticker, pct_change);
                }
            }
        },
        None => {}
    }
}

fn calc_net_worth(cash: f32, holdings: HashMap<String, f32>) -> f32 {
    let mut net_worth = cash;
    let provider = yahoo::YahooConnector::new();

    for (h, q) in holdings.iter() {
        let response = tokio_test::block_on(provider.get_latest_quotes(h, "1d")).unwrap();
        let quote = response.last_quote().unwrap();
        net_worth = net_worth + (*q * quote.close as f32)
    }
    net_worth
}

fn calc_net_worth_with_closes(cash: f32, holdings: HashMap<String, f32>, closes: HashMap<String, f32>) -> f32 {
    let mut net_worth = cash;
    for (h, q) in holdings.iter() {
        let close = closes.get(h).unwrap();
        net_worth += *q * close;
    }
    net_worth  
}

fn get_closes_for_tickers(tickers: &Vec<String>) -> HashMap<String, f32> {
    let provider = yahoo::YahooConnector::new();
    let mut closes: HashMap<String, f32> = HashMap::new();
    for t in tickers.iter() {
        let response = tokio_test::block_on(provider.get_latest_quotes(t, "1d")).unwrap();
        let quote = response.last_quote().unwrap();
        closes.insert(t.to_string(), quote.close as f32);
    }
    closes
}

fn get_quotes_for_tickers(tickers: &Vec<String>) -> Vec<TickeredQuote> {
    let mut all_quotes: Vec<TickeredQuote> = Vec::new();
    for t in tickers.iter() {
        let provider = yahoo::YahooConnector::new();
        let response = tokio_test::block_on(provider.get_quote_history(t, START, END)).unwrap();
        let quotes = response.quotes().unwrap();
        for q in quotes.iter() {
            all_quotes.push( TickeredQuote{
                ticker: t.to_string(),
                quote: q.clone(),
            })
        }
    }
    all_quotes.sort_by_key(|k| k.quote.timestamp);
    all_quotes
}

fn calc_buy_and_hold_strategy(tickers: &Vec<String>) -> f32 {
    let provider = yahoo::YahooConnector::new();
    let amount_per_ticker = STARTING_CASH / tickers.len() as f32;
    let mut holdings: HashMap<String, f32> = HashMap::new();
    for t in tickers.iter() {
        let response = tokio_test::block_on(provider.get_quote_history(t, START, END)).unwrap();
        let quotes = response.quotes().unwrap();
        let q = quotes.first().unwrap();
        let amount_to_buy = amount_per_ticker / q.close as f32;
        holdings.insert(t.to_string(), amount_to_buy);

    }
    calc_net_worth(0.0, holdings)
}

fn simulate(quotes: &Vec<TickeredQuote>, buy_increment: f32, buy_when: f32, sell_when: f32) -> (HashMap<String, f32>, f32, f32) {
    let mut holdings: HashMap<String, f32> = HashMap::new();
    let mut purchases: HashMap<String, HashMap<DateTime<Utc>, f32>> = HashMap::new();
    let mut cash_spent: f32 = 0.0;
    let mut cash_made: f32 = 0.0;
    let mut prev_closes: HashMap<String, f32> = HashMap::new();
    let cash_per_purchase: f32 = buy_increment;
    let cash_limit: f32 = STARTING_CASH;
    for q in quotes.iter() {
        let date = Utc.timestamp_opt(q.quote.timestamp as i64, 0).unwrap();
        let prev_close = match prev_closes.get(&q.ticker) {
            Some(a) => a,
            None => {
                prev_closes.insert(q.ticker.clone(), q.quote.close as f32);
                continue;
            },          
        };
        let pct: f32 = (q.quote.close as f32 - prev_close) * 100.0 / prev_close;
        if pct <= -1.0 * buy_when && cash_spent + cash_per_purchase - cash_made < cash_limit { // buy low
            let amount_to_buy = cash_per_purchase / (q.quote.close as f32);
            cash_spent += cash_per_purchase;
            match holdings.get(&q.ticker) {
                Some(amount_owned) => holdings.insert(q.ticker.clone(), amount_owned + amount_to_buy),
                None => holdings.insert(q.ticker.clone(), amount_to_buy),
            };
            let mut purchase_map: HashMap<DateTime<Utc>, f32> = purchases.get(&q.ticker).unwrap_or(&HashMap::new()).clone();
            purchase_map.insert(date, amount_to_buy);
            purchases.insert(q.ticker.clone(), purchase_map.clone());
        };
        if pct >= sell_when { // sell high
            let mut to_remove: Vec<DateTime<Utc>> = Vec::new();
            let mut purchase_map: HashMap<DateTime<Utc>, f32> = purchases.get(&q.ticker).unwrap_or(&HashMap::new()).clone();
            for (k, v) in purchase_map.iter() {
                if *k <= date - Duration::from_secs(60 * 60 * 24 * 365) { // now - 1yr
                    // we can sell
                    cash_made += *v * q.quote.close as f32;
                    holdings.insert(q.ticker.clone(),holdings.get(&q.ticker).unwrap() - *v);
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

fn get_latest_change(ticker: &String) -> f32 {
    let provider = yahoo::YahooConnector::new();
    let start = time::OffsetDateTime::now_utc() - Duration::from_secs(60 * 60 * 24 * 7); // now - 1wk
    let end = time::OffsetDateTime::now_utc();
    let response = tokio_test::block_on(provider.get_quote_history(ticker, start, end)).unwrap();
    let quotes = response.quotes().unwrap();
    let previous = quotes.iter().nth(quotes.len() - 2).unwrap();
    let current = quotes.iter().last().unwrap();
    let pct_change = (current.close as f32 - previous.close as f32) * 100.0 / previous.close as f32;
    println!("[{}]: Got last day pct_change: {}", ticker, pct_change);
    pct_change
}

fn send_alert_email(ticker: &String, pct_change: f32) {
    let email = Message::builder() 
    .from(env::var("EMAIL_ADDRESS").unwrap().parse().unwrap()) 
    .to(env::var("EMAIL_ADDRESS").unwrap().parse().unwrap()) 
    .subject("Alert from stocktrader-rs") 
    .body(String::from(format!("Stock: {}, Change: {}", ticker, pct_change))) 
    .unwrap(); 
  
  let creds = Credentials::new(env::var("EMAIL_ADDRESS").unwrap().to_string(), env::var("SMTP_PASSWORD").unwrap().to_string()); 
  
  // Open a remote connection to gmail 
  let mailer = SmtpTransport::relay("smtp.gmail.com") 
    .unwrap() 
    .credentials(creds) 
    .build(); 
  
  // Send the email 
  match mailer.send(&email) { 
    Ok(_) => println!("Email sent successfully!"), 
    Err(e) => panic!("Could not send email: {:?}", e), 
  }
}