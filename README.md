# stocktrader-rs
An awful stock automation cli, in rust.

## build it
`cargo build -r`

## run it?
```
./target/release/trader -h
Usage: trader [COMMAND]

Commands:
  quote     
  simulate  
  alert     
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

### quote

Gets a history of daily quotes for a provided ticker, optionally filtered by days that gain/loss more than a provided percentage:
```
./target/release/trader quote -t GOOGL -f 5
got quotes for 1258 days
date        | close      | pct      
2019-02-15  |   55.98 |    inf
2019-04-30  |   59.95 |  -7.50
2019-06-03  |   51.94 |  -6.12
2019-07-26  |   62.26 |   9.62
...
2024-01-31  |  140.10 |  -7.50
```

### simulate

Simulates multiple buying/selling strategies, optimizing for
  * buy_when: how much the stock loses in a single day to trigger a buy
  * sell_when: how much the stock gains in a single day to trigger a sell
  * buy_pct: how much of the available cash to buy with
By default operates over 5yr of daily close prices.
Can provide multiple tickers to consider.
```
./target/debug/trader simulate --tickers GOOGL --tickers AMZN --tickers MSFT
Hit best_net_worth: 161183.92, buy_when: 1.0, sell_when: 1.0, buy_pct: 0.1
Hit best_net_worth: 201417.27, buy_when: 1.0, sell_when: 1.0, buy_pct: 0.2
Hit best_net_worth: 222928.31, buy_when: 1.0, sell_when: 1.0, buy_pct: 0.3
Hit best_net_worth: 231473.88, buy_when: 1.0, sell_when: 1.0, buy_pct: 0.4
...
Hit best_net_worth: 630542.0, buy_when: 2.5, sell_when: 3.1000001, buy_pct: 0.8
```

### alert
Email an alert when buy/sell thresholds are met.

```
./target/release/trader alert --tickers GOOGL --buy-when 1.0 --sell-when 1.0
[GOOGL]: Got last day pct_change: -1.5759612
Email sent successfully!
```

requires env vars:
```
export EMAIL_ADDRESS=<YOUR-EMAIL-ADDRESS>
export SMTP_PASSWORD=<YOUR-GOOGLE-APP-CREDENTIALS>
```
see: https://support.google.com/accounts/answer/185833 for app password creation.


