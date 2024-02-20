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
  * `buy_when`: how much the stock loses in a single day to trigger a buy
  * `sell_when`: how much the stock gains in a single day to trigger a sell
  * `buy_increment`: how much cash to spend on each triggerring purchase

By default operates over 10 years of daily close prices.

Obeys the following rules:
  * stocks bought cannot be resold within 1yr, for short-term cap gains tax reasons
  * maximum $100,000 cash debt; selling replenishes cash debt.

Provides a baseline `Buy & Hold` strategy for comparison.

Can provide multiple tickers to consider:
```
./target/debug/trader simulate --tickers GOOGL --tickers MSFT --tickers AMZN
Best Net-Worth: $1170030.88, buy_when: -4.8%, sell_when: 5.0%, buy_increment: $33333.00
Buy & Hold: $823931.75
```

### alert
Email an alert when buy/sell thresholds are met:

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


