use backtesting::{BacktestEngine, MarketDataBar, SMAStrategy, OHLCV};
use rand::{rngs::StdRng, Rng, SeedableRng};

fn main() {
    println!("=== Quantitative Trading System - Backtesting Demo ===\n");

    // Generate synthetic market data (seeded, so every run prints the same numbers)
    let market_data = generate_market_data(42);
    println!("Generated {} bars of market data", market_data.len());

    // Create strategy
    let strategy = Box::new(SMAStrategy::new(
        "BTC".to_string(),
        10, // Fast SMA period
        30, // Slow SMA period
    ));

    // Create backtest engine
    let mut engine = BacktestEngine::new(
        strategy, 100_000.0, // Initial capital
        0.001,     // Commission rate (0.1%)
        0.0001,    // Slippage (0.01%)
    );

    // Run backtest
    println!("\nRunning backtest...");
    engine.run(market_data).unwrap();

    // Get results
    let metrics = engine.get_results().unwrap();
    let equity_curve = engine.get_equity_curve();
    let trades = engine.get_trades();

    // Print performance metrics
    println!("\n=== Performance Metrics ===");
    println!("Total Return: {:.2}%", metrics.total_return * 100.0);
    println!("Annualized Return: {:.2}%", metrics.annualized_return);
    println!("Sharpe Ratio: {:.2}", metrics.sharpe_ratio);
    println!("Sortino Ratio: {:.2}", metrics.sortino_ratio);
    println!("Calmar Ratio: {:.2}", metrics.calmar_ratio);
    println!("Max Drawdown: {:.2}%", metrics.max_drawdown);
    println!(
        "Max DD Duration: {:.1} days",
        metrics.max_drawdown_duration_days
    );

    println!("\n=== Trade Statistics ===");
    println!("Total Trades: {}", metrics.total_trades);
    println!(
        "Winning Trades: {} ({:.1}%)",
        metrics.winning_trades,
        metrics.win_rate * 100.0
    );
    println!("Losing Trades: {}", metrics.losing_trades);
    println!("Profit Factor: {:.2}", metrics.profit_factor);
    println!("Average Win: ${:.2}", metrics.average_win);
    println!("Average Loss: ${:.2}", metrics.average_loss);
    println!("Largest Win: ${:.2}", metrics.largest_win);
    println!("Largest Loss: ${:.2}", metrics.largest_loss);

    println!("\n=== Risk Metrics ===");
    println!("Volatility (Annual): {:.2}%", metrics.volatility);
    println!("Downside Volatility: {:.2}%", metrics.downside_volatility);
    println!("VaR (95%): ${:.2}", metrics.var_95);
    println!("VaR (99%): ${:.2}", metrics.var_99);
    println!("CVaR (95%): ${:.2}", metrics.cvar_95);
    println!("CVaR (99%): ${:.2}", metrics.cvar_99);

    println!("\n=== Final Portfolio ===");
    let portfolio = engine.get_portfolio();
    println!("Final Equity: ${:.2}", portfolio.total_equity());
    println!("Cash: ${:.2}", portfolio.cash);
    println!("Open Positions: {}", portfolio.positions.len());

    println!("\n=== Equity Curve (Last 10 points) ===");
    for point in equity_curve.iter().rev().take(10).rev() {
        println!("  ${:.2}", point.equity);
    }

    println!("\n=== Recent Trades (Last 5) ===");
    for trade in trades.iter().rev().take(5) {
        println!(
            "  {:?} {} @ ${:.2} | P&L: ${:.2}",
            trade.side, trade.quantity, trade.price, trade.pnl
        );
    }

    println!("\nBacktest completed.");
}

fn generate_market_data(seed: u64) -> Vec<MarketDataBar> {
    let mut rng = StdRng::seed_from_u64(seed);

    let mut data = Vec::new();
    let mut price = 50000.0;

    for i in 0..500u64 {
        // Random walk with trend
        let change = rng.gen_range(-0.02..0.03);
        price *= 1.0 + change;

        let high = price * (1.0 + rng.gen_range(0.0..0.01));
        let low = price * (1.0 - rng.gen_range(0.0..0.01));
        let open = price + rng.gen_range(-100.0..100.0);

        data.push(MarketDataBar {
            timestamp: i * 86400 * 1_000_000_000, // Daily bars
            symbol: "BTC".to_string(),
            ohlcv: OHLCV {
                timestamp: i * 86400 * 1_000_000_000,
                open,
                high,
                low,
                close: price,
                volume: rng.gen_range(100.0..1000.0),
            },
        });
    }

    data
}
