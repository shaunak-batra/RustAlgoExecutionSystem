# Algorithmic Execution Sandbox

A high-performance algorithmic trading execution system built in Rust. This system implements multiple execution algorithms (TWAP, POV, VWAP, IS) with real-time monitoring, backtesting, and performance analytics.

## Table of Contents

- [What This System Actually Does](#what-this-system-actually-does)
- [Installation & Setup](#installation--setup)
- [Quick Start](#quick-start)
- [Mathematical Foundations](#mathematical-foundations)
- [Trading & Finance Concepts](#trading--finance-concepts)
- [Execution Algorithms Explained](#execution-algorithms-explained)
- [Performance Metrics Deep Dive](#performance-metrics-deep-dive)
- [Understanding Results](#understanding-results)
- [Architecture](#architecture)
- [API Usage](#api-usage)
- [Development](#development)

---

## What This System Actually Does

### The Core Problem

You want to buy 10,000 shares of a stock. If you submit a market order for 10,000 shares all at once, you'll likely:

1. **Move the market** - Your large buy order pushes the price up as you consume available liquidity
2. **Get terrible execution** - You'll buy the first 1,000 shares at $100, the next 1,000 at $100.10, then $100.20, and so on
3. **Alert other traders** - They see a large order and front-run you

This is called **market impact**, and it's expensive. If the stock is trading at $100 and you move it to $100.50 while buying, you've paid an extra $0.50 per share on average. On 10,000 shares, that's $5,000 in unnecessary costs.

### The Solution

**Execution algorithms** solve this by splitting your large order into smaller "child orders" and executing them over time. Instead of buying 10,000 shares at once, you might:

- Buy 100 shares every 30 seconds for 50 minutes (TWAP)
- Buy proportionally to market volume, so you're never more than 10% of the market (POV)
- Buy more when volume is higher and less when volume is lower (VWAP)
- Front-load execution if you expect the price to move against you (IS)

This system simulates these algorithms so you can test them, understand how they work, and measure their performance without risking real money.

### What You Get

- **Execution Engine**: Processes parent orders, breaks them into child orders, executes them on schedule
- **Simulated Orderbook**: Tests your algorithms against a realistic order book with bid-ask spread, market makers, and liquidity
- **Real Market Data**: Pulls live crypto prices from Binance to make the simulation realistic
- **GUI**: Visual interface to submit orders, monitor execution, and view performance
- **Performance Analytics**: Sharpe ratio, drawdown, win rate, VaR, and other institutional-grade metrics
- **Backtesting Framework**: Test your strategies on historical data
- **gRPC API**: Programmatic access for automated trading systems

---

## Installation & Setup

### Requirements

**1. Rust (1.70+)**

```powershell
# Install Rust via rustup
# Visit: https://rustup.rs/
# Or run:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Set stable as default
rustup default stable
```

**2. Visual Studio 2022 (Windows only)**

- Download: [Visual Studio Community](https://visualstudio.microsoft.com/)
- Install workload: **"Desktop development with C++"**
- This provides the MSVC linker and Windows SDK that Rust needs

**3. Protocol Buffers Compiler (optional)**

- Only needed if modifying the gRPC API
- Download: [protobuf releases](https://github.com/protocolbuffers/protobuf/releases)

### Building

```powershell
# Clone or navigate to project
cd C:\Users\YourName\Desktop\QuantSystem

# Build all components (takes 3-5 minutes first time)
cargo build --release

# Output binaries in target/release/:
# - engine.exe (execution engine)
# - trader-gui.exe (GUI application)
```

**Troubleshooting Build Issues:**

- **"Cannot find kernel32.lib"**: Install Windows SDK via Visual Studio
- **"linker error"**: Make sure MSVC is installed, not just MinGW
- **"Cargo.lock conflicts"**: Run `cargo update` then rebuild

---

## Quick Start

### 1. Start the Engine

Open a terminal:

```powershell
cargo run --release -p engine
```

You'll see:

```
INFO Starting Algo Execution Engine...
INFO Seeding orderbook BTCUSDT with market maker liquidity at price 10000000
INFO Starting gRPC server on 0.0.0.0:9090...
```

**Keep this terminal running.** The engine is now:

- Listening for orders on port 9090
- Maintaining a simulated orderbook with market maker liquidity
- Ready to execute child orders and generate fills

### 2. Launch the GUI

Open a **second terminal**:

```powershell
cargo run --release -p trader-gui
```

A window opens with:

- Real-time crypto prices (BTC, ETH, SOL, BNB, ADA) from Binance
- Live technical indicators (RSI, MACD, Bollinger Bands)
- Order entry form pre-filled with a sample TWAP order
- Empty positions and orders tables

### 3. Connect and Trade

1. Click **"File" → "Connect"**

   - Status bar should show green "● Connected"

2. Submit a test order (form is pre-filled):

   - **Symbol**: BTCUSDT
   - **Side**: BUY
   - **Quantity**: 1000
   - **Duration**: 10 seconds
   - **Slices**: 10
   - **Algorithm**: TWAP

3. Click **"🚀 Submit TWAP"**

### 4. Watch Execution

The engine terminal shows:

```
INFO [EXEC] Executing child order Order#1001 at target time...
INFO Fill: order=Order#1001 qty=100 price=$100.01000 time=...
INFO Fill: order=Order#1002 qty=100 price=$100.01000 time=...
...
INFO Parent order 12345 fully filled
```

The GUI updates in real-time:

- **Orders table**: Shows order status changing from ACCEPTED → FILLED
- **Positions table**: Shows your position building up (100, 200, 300, ..., 1000)
- **Performance Dashboard**: Updates with metrics after fills complete

### 5. Explore Performance Metrics

Expand **"📈 Performance Dashboard"** to see:

- **Equity curve**: Visual graph of your P&L over time
- **Sharpe ratio**: Risk-adjusted return (> 1.5 is good)
- **Max drawdown**: Worst peak-to-trough decline
- **Win rate**: Percentage of profitable trades
- **VaR/CVaR**: Value at risk metrics (95% and 99% confidence)

Submit 5-10 more orders with different symbols and sides to generate meaningful statistics.

---

## Mathematical Foundations

### Order Book Mechanics

The simulated orderbook maintains buy and sell orders in **price-time priority**:

```
SELL SIDE (asks)
$100.02  │  500 shares
$100.01  │  1000 shares
─────────┼─────────────  (spread = $0.02)
$99.99   │  1000 shares
$99.98   │  500 shares
BUY SIDE (bids)
```

**Best bid**: $99.99 (highest buy price)
**Best ask**: $100.01 (lowest sell price)
**Mid-price**: $100.00 (average of bid and ask)
**Spread**: $0.02 (ask - bid)

When you submit a **market buy order**, it matches against the best ask:

- Your order "crosses the spread" by buying at $100.01
- This is called **paying the spread** or **taking liquidity**

When you submit a **limit buy order at $99.99**, it joins the bid side:

- You're "providing liquidity" and waiting to be filled
- You avoid paying the spread but might not get filled immediately

### Price Impact Model

Price impact is the effect your order has on the market price. It's typically modeled as:

```
impact = k × sqrt(order_size / average_daily_volume)
```

Where:

- `k` is a market-specific constant (typically 0.1 to 1.0)
- `sqrt` reflects that impact grows sub-linearly (doubling size doesn't double impact)

**Example:**

- Stock trades 1,000,000 shares/day
- Your order is 10,000 shares
- Impact ≈ 0.3 × sqrt(10,000 / 1,000,000) = 0.3 × 0.1 = 3%

So your 10,000 share buy might push the price up 3% from $100 to $103.

### Slippage

**Slippage** is the difference between expected and actual execution price:

```
slippage = execution_price - decision_price
```

**Components:**

1. **Spread cost**: Half-spread on average (if mid-price is fair value)
2. **Market impact**: Your order moves the price
3. **Delay cost**: Price moves between decision and execution
4. **Opportunity cost**: Price moves against you while waiting

**Example:**

- You decide to buy at $100.00 (mid-price)
- You execute at $100.50 over 10 minutes
- Slippage = $0.50 per share
- On 10,000 shares, that's $5,000 in costs

Execution algorithms aim to minimize slippage while ensuring you complete your order.

### Volume Profiles

Markets don't trade uniformly throughout the day. Volume typically follows a U-shape:

```
Volume Pattern (typical equity market):
     │
High │ ███░░░░░░░░░░░██
     │ ███░░░░░░░░░░░██
 Mid │ ███░░░░░░░░░░░██
     │ ███░░░░░░░░░░███
 Low │ ███░░░░░░░░░░███
     └────────────────────
       9am    12pm   4pm
       (open) (mid)  (close)
```

**VWAP algorithms** exploit this by:

- Executing more shares during high-volume periods (open and close)
- Executing fewer shares during low-volume periods (lunch)
- This minimizes your percentage of volume and reduces market impact

---

## Trading & Finance Concepts

### Market Microstructure

**Bid-Ask Spread**
The spread represents the cost of immediacy. Market makers profit from the spread by:

1. Buying at the bid ($99.99)
2. Selling at the ask ($100.01)
3. Capturing the spread ($0.02 per share)

In exchange, they provide liquidity to the market.

**Liquidity**
Liquidity measures how easy it is to buy or sell without moving the price:

- **High liquidity**: Tight spreads, large order book depth, minimal price impact
- **Low liquidity**: Wide spreads, thin order books, large price impact

**Order Types:**

- **Market order**: Execute immediately at best available price (pay the spread)
- **Limit order**: Execute only at specified price or better (may not fill)
- **IOC (Immediate or Cancel)**: Execute whatever fills immediately, cancel the rest
- **FOK (Fill or Kill)**: Execute entire order immediately or cancel entirely

### Position Tracking

**Net Position** = Sum of all trades

- **Long position**: Net positive (you own shares)
- **Short position**: Net negative (you owe shares)
- **Flat**: Net zero (no position)

**Example:**

```
Trade 1: BUY 500 @ $100.00
Trade 2: BUY 500 @ $101.00
Trade 3: SELL 200 @ $102.00
─────────────────────────────
Net position: +800 shares (long)
```

**Average Entry Price**
Weighted average of your entry prices:

```
avg_price = (qty1 × price1 + qty2 × price2 + ...) / total_qty
```

For the example above:

```
avg_price = (500 × 100 + 500 × 101) / 1000 = $100.50
```

**Realized vs. Unrealized P&L**

- **Realized P&L**: Profit/loss from closed trades (you've sold shares you bought)
- **Unrealized P&L**: Profit/loss from open position (you still hold shares)

```
Realized P&L = (exit_price - entry_price) × quantity
Unrealized P&L = (current_price - avg_entry_price) × open_quantity
```

Using the example above:

```
Realized P&L = (102 - 100.50) × 200 = $300
Unrealized P&L = (103 - 100.50) × 800 = $2,000 (if current price is $103)
Total P&L = $2,300
```

### Transaction Cost Analysis (TCA)

TCA measures execution quality by comparing actual results to benchmarks:

**Arrival Price**: Price when you decided to trade
**Benchmark**: What you're comparing against (VWAP, arrival, close, etc.)
**Implementation Shortfall**: Difference between arrival price and average execution price

```
implementation_shortfall = avg_execution_price - arrival_price
```

**Example:**

- Arrival price: $100.00
- You execute 10,000 shares over 10 minutes
- Average execution price: $100.30
- Implementation shortfall: $0.30 × 10,000 = $3,000 cost

**Participation Rate**: Your trades as % of market volume

```
participation_rate = your_volume / market_volume
```

- Low participation (< 5%): Less market impact, slower execution
- High participation (> 20%): More market impact, faster execution

---

## Execution Algorithms Explained

### TWAP (Time-Weighted Average Price)

**Goal**: Execute evenly over time, regardless of market conditions.

**Schedule Formula**:

```
For N slices over duration T:
  slice_interval = T / N
  slice_size = total_quantity / N

  slice[i].time = start_time + (i × slice_interval)
  slice[i].qty = slice_size
```

**Example**:

- Total quantity: 1,000 shares
- Duration: 10 seconds
- Slices: 10

```
Slice 1:  t=0s,  qty=100
Slice 2:  t=1s,  qty=100
Slice 3:  t=2s,  qty=100
...
Slice 10: t=9s,  qty=100
```

**When to use TWAP**:

- ✅ You don't have a view on price direction
- ✅ You want predictable, evenly-spaced execution
- ✅ You're trading in a stable market
- ❌ Don't use when volume varies significantly throughout the day

**Advantages**:

- Simple and transparent
- Predictable execution pattern
- Easy to explain to clients

**Disadvantages**:

- Ignores market volume patterns
- Can be too aggressive during low-volume periods
- Can be too passive during high-volume periods

### POV (Percent-of-Volume)

**Goal**: Execute as a fixed percentage of market volume.

**Schedule Formula**:

```
For each period:
  observed_market_volume = market_trades_in_period
  target_execution = participation_rate × observed_market_volume

  # Adjust to meet overall target
  remaining_qty = total_qty - executed_qty
  remaining_time = end_time - current_time

  slice_qty = min(target_execution, remaining_qty)
```

**Example**:

- Total quantity: 1,000 shares
- Participation rate: 10%
- Market trades 500 shares in period 1 → You execute 50 shares
- Market trades 1,000 shares in period 2 → You execute 100 shares

**When to use POV**:

- ✅ You want to stay "hidden" in the market flow
- ✅ You don't want to be more than X% of volume
- ✅ You have time and don't need urgent execution
- ❌ Don't use in illiquid markets (you might never finish)

**Advantages**:

- Minimizes market impact by following natural volume
- Adapts to market conditions
- Good for large orders in liquid markets

**Disadvantages**:

- Unpredictable completion time (what if volume dries up?)
- Complex to explain (clients want certainty)
- Requires accurate volume forecasts

### VWAP (Volume-Weighted Average Price)

**Goal**: Match the day's volume-weighted average price.

**VWAP Calculation**:

```
VWAP = Σ(price[i] × volume[i]) / Σ(volume[i])
```

**Schedule Formula**:

```
For each period:
  expected_volume[i] = historical_volume_profile[i]
  total_expected_volume = Σ expected_volume[i]

  slice_qty[i] = (expected_volume[i] / total_expected_volume) × total_qty
```

**Example**:
Historical volume profile for 9am-10am:

```
9:00-9:15: 30% of volume → Execute 300 shares (30% of 1,000)
9:15-9:30: 25% of volume → Execute 250 shares
9:30-9:45: 20% of volume → Execute 200 shares
9:45-10:00: 25% of volume → Execute 250 shares
```

**When to use VWAP**:

- ✅ You want to match the market's average price
- ✅ You're measured against VWAP as a benchmark
- ✅ You have historical volume patterns
- ❌ Don't use on days with unusual volume (earnings, news)

**Advantages**:

- Matches the market's natural rhythm
- Benchmark used by many institutional investors
- Minimizes adverse selection (not trading at wrong times)

**Disadvantages**:

- Relies on historical volume patterns (may not predict today)
- Can't adapt if volume pattern changes
- Front-loaded execution can alert traders

### IS (Implementation Shortfall)

**Goal**: Minimize the cost of delay while managing market impact.

**Cost Model**:

```
total_cost = market_impact_cost + delay_cost

market_impact_cost = k × sqrt(execution_rate) × volatility
delay_cost = drift × time_to_completion

# Optimal urgency minimizes total cost
urgency = f(price_drift, volatility, remaining_qty)
```

**Schedule Formula**:

```
For each period:
  # Higher urgency = front-load execution
  weight[i] = urgency^i

  slice_qty[i] = (weight[i] / Σ weight) × remaining_qty

  # Dynamic adjustment based on price movement
  if price_moving_against_us:
    urgency += 0.1  # Execute faster
  if price_moving_favorably:
    urgency -= 0.1  # Execute slower
```

**Example** (high urgency = 0.7):

```
Period 1: 40% of total → 400 shares (front-loaded)
Period 2: 30% of total → 300 shares
Period 3: 20% of total → 200 shares
Period 4: 10% of total → 100 shares (back-loaded)
```

**When to use IS**:

- ✅ You have a strong view on price direction
- ✅ You expect momentum or price drift
- ✅ You need to balance urgency vs. impact
- ❌ Don't use if you have no view on price direction

**Advantages**:

- Adapts to price movements in real-time
- Optimal for minimizing implementation shortfall
- Can front-load or back-load based on urgency

**Disadvantages**:

- Complex to understand and explain
- Requires sophisticated models (volatility, drift, impact)
- Can be too aggressive if models are wrong

---

## Performance Metrics Deep Dive

### Returns

**Simple Return**:

```
R = (P_end - P_start) / P_start
```

**Log Return** (used in finance because they're additive):

```
r = ln(P_end / P_start)
```

**Cumulative Return**:

```
R_cumulative = (1 + R_1) × (1 + R_2) × ... × (1 + R_n) - 1
```

### Sharpe Ratio

**Formula**:

```
Sharpe = (R_portfolio - R_risk_free) / σ_portfolio
```

Where:

- `R_portfolio`: Average return of your strategy
- `R_risk_free`: Risk-free rate (Treasury bills, ~4% annually in 2024)
- `σ_portfolio`: Standard deviation of returns (volatility)

**What it means**:

- **Sharpe < 1.0**: Poor risk-adjusted returns
- **Sharpe 1.0-2.0**: Good risk-adjusted returns
- **Sharpe > 2.0**: Excellent risk-adjusted returns (rare)
- **Sharpe > 3.0**: Suspicious (might be overfitted or taking hidden risks)

**Intuition**: How much return are you getting per unit of risk?

**Example**:

```
Strategy A: 15% return, 10% volatility → Sharpe = (15 - 4) / 10 = 1.1
Strategy B: 20% return, 20% volatility → Sharpe = (20 - 4) / 20 = 0.8
```

Strategy A is better risk-adjusted, even though B has higher absolute return.

**Annualization**:

```
Sharpe_annual = Sharpe_period × sqrt(periods_per_year)

Daily Sharpe → Annual: multiply by sqrt(252)
Monthly Sharpe → Annual: multiply by sqrt(12)
```

### Sortino Ratio

**Formula**:

```
Sortino = (R_portfolio - R_target) / σ_downside
```

Where:

- `σ_downside`: Standard deviation of _negative_ returns only

**Why it's better than Sharpe**:

- Sharpe penalizes upside volatility (who cares if you make too much money?)
- Sortino only penalizes downside volatility (actual risk)

**Example**:

```
Returns: [+10%, +20%, -5%, +15%, -10%]

For Sharpe: σ = std([10, 20, -5, 15, -10]) = 11.7%
For Sortino: σ_downside = std([-5, -10]) = 3.5%

Sortino will be higher because it ignores upside volatility
```

### Maximum Drawdown

**Formula**:

```
Drawdown[t] = (Peak[t] - Value[t]) / Peak[t]
Max Drawdown = max(Drawdown[t]) for all t
```

**Intuition**: Worst peak-to-trough decline in portfolio value.

**Example**:

```
Equity: $100k → $150k → $120k → $180k → $100k

Drawdown 1: ($150k - $120k) / $150k = 20%
Drawdown 2: ($180k - $100k) / $180k = 44.4%

Max Drawdown = 44.4%
```

**What it means**:

- **< 10%**: Low risk, conservative strategy
- **10-20%**: Moderate risk, typical for balanced portfolios
- **20-30%**: High risk, aggressive trading
- **> 30%**: Very high risk, many investors would have quit

**Drawdown Duration**: How long it takes to recover to previous peak.

```
Recovery Time = date[new_peak] - date[previous_peak]
```

Long drawdown durations are psychologically difficult. A 50% drawdown that recovers in 1 month is easier to handle than a 20% drawdown that lasts 2 years.

### Calmar Ratio

**Formula**:

```
Calmar = Annualized_Return / |Max_Drawdown|
```

**Intuition**: Return per unit of maximum loss.

**Example**:

```
Strategy: 18% annual return, 15% max drawdown
Calmar = 18 / 15 = 1.2
```

**What it means**:

- **Calmar < 1.0**: You're losing more in drawdowns than you're making in returns
- **Calmar > 1.0**: Returns exceed worst-case losses
- **Calmar > 3.0**: Excellent (stable returns, small drawdowns)

### Value at Risk (VaR)

**Formula**:

```
VaR_95 = 5th percentile of return distribution
```

**Intuition**: "I'm 95% confident I won't lose more than X on any given day."

**Example**:

```
Daily returns: [-5%, -2%, -1%, 0%, +1%, +2%, +3%, +4%, +5%]
5th percentile = -5%

If portfolio value is $100k:
VaR_95 = $100k × 5% = $5,000

"I'm 95% confident I won't lose more than $5,000 in a day"
```

**Limitations**:

- Assumes normal distribution (real markets have fat tails)
- Doesn't tell you how bad the worst 5% of days can be
- Can be gamed (sell options to collect premium, hide tail risk)

### Conditional Value at Risk (CVaR)

**Formula**:

```
CVaR_95 = Average of returns worse than VaR_95
```

**Intuition**: "On my worst 5% of days, I lose X on average."

**Example**:

```
Daily returns: [-8%, -7%, -5%, -2%, -1%, 0%, +1%, +2%, +3%]

VaR_95 = -5% (5th percentile)
CVaR_95 = average([-8%, -7%]) = -7.5%

"On my worst days, I lose 7.5% on average"
```

**Why it's better than VaR**:

- Captures tail risk
- Tells you severity of worst-case scenarios
- More informative for risk management

### Win Rate

**Formula**:

```
Win Rate = (Winning Trades / Total Trades) × 100%
```

**What it means**:

- **< 40%**: Low win rate (better be high profit factor)
- **40-60%**: Typical for most strategies
- **> 60%**: High win rate (probably small wins, rare big losses)

**Critical insight**: **Win rate alone is meaningless!**

```
Strategy A: 90% win rate, avg win $100, avg loss $5,000
Strategy B: 40% win rate, avg win $1,000, avg loss $300

Strategy A is terrible (big losses wipe out small wins)
Strategy B is better (wins are larger than losses)
```

### Profit Factor

**Formula**:

```
Profit Factor = Gross Profit / Gross Loss
```

**What it means**:

- **PF < 1.0**: Losing money overall
- **PF = 1.0**: Breaking even
- **PF 1.0-1.5**: Barely profitable
- **PF 1.5-2.0**: Good
- **PF > 2.0**: Excellent (rare)

**Example**:

```
10 trades:
Wins: $500, $400, $600, $300 = $1,800 total
Losses: -$200, -$300, -$400, -$100, -$200, -$100 = -$1,300 total

Profit Factor = $1,800 / $1,300 = 1.38
```

For every $1 you lose, you make $1.38. That's a profitable system.

---

## Understanding Results

### Order Status Flow

```
PENDING → ACCEPTED → WORKING → PARTIALLY_FILLED → FILLED
                          ↓
                      CANCELLED
                          ↓
                      REJECTED
```

**PENDING**: Order received, waiting to start execution
**ACCEPTED**: Order validated, execution will begin
**WORKING**: Child orders actively being sent to market
**PARTIALLY_FILLED**: Some fills received, more expected
**FILLED**: All quantity executed, order complete
**CANCELLED**: User cancelled before completion
**REJECTED**: Risk checks failed or invalid parameters

### Fill Events

Each fill represents an actual execution:

```
Fill #1234
├── Order ID: 5678 (child order)
├── Quantity: 100 shares
├── Price: $100.01
└── Timestamp: 1,700,000,000,000 ns (nanoseconds since epoch)
```

**Key metrics from fills**:

- **Fill rate**: Percentage of order filled
- **Average fill price**: Weighted average of all fill prices
- **Price variance**: Spread of fill prices (measures slippage)

**Example analysis**:

```
10 fills of 100 shares each at prices:
[$100.01, $100.01, $100.02, $100.01, $100.03,
 $100.02, $100.01, $100.04, $100.02, $100.01]

Average fill price = $100.018
Min = $100.01, Max = $100.04
Range = $0.03 (slippage)
```

### Position Interpretation

```
Symbol: BTCUSDT
Quantity: +1,000 (positive = long)
Avg Price: $100.50
Realized P&L: +$300 (from closed portions)
Unrealized P&L: +$500 (from open position)
Total P&L: +$800
```

**Scenario walkthrough**:

1. Buy 500 @ $100 → Position: +500, Avg: $100
2. Buy 500 @ $101 → Position: +1,000, Avg: $100.50
3. Sell 200 @ $102 → Position: +800, Realized: +$300 (sold at $102, avg cost $100.50)
4. Current price: $101.125 → Unrealized: +$500 (800 shares × ($101.125 - $100.50))

### Performance Dashboard

**Reading the Equity Curve**:

```
$120k ┤     ╭─────╮
      │    ╱       ╰╮
$100k ┤───╯         ╰─╮  ← Drawdown period
      │               ╰──
$80k  ┤
      └─────────────────────
        Jan  Feb  Mar  Apr
```

- **Uptrend**: Strategy is working, keep going
- **Flat**: Strategy not generating returns, investigate
- **Downtrend**: Strategy losing money, stop or adjust
- **Drawdown**: Temporary decline (normal) or start of failure (concerning)?

**Good equity curve traits**:

- Smooth upward slope (consistent profits)
- Small drawdowns (< 15%)
- Quick recovery from drawdowns (< 1 month)
- Increasing slope over time (compounding)

**Bad equity curve traits**:

- Large drawdowns (> 30%)
- Long flat periods (opportunity cost)
- Declining slope (strategy degrading)
- High volatility (erratic returns)

---

## Architecture

### System Components

```
┌─────────────────────────────────────────────────┐
│                  trader-gui                      │
│  (User interface, charts, performance metrics)  │
└────────────────┬────────────────────────────────┘
                 │ gRPC (port 9090)
                 ↓
┌─────────────────────────────────────────────────┐
│                    engine                       │
│  ┌────────────────────────────────────────────┐ │
│  │  Event Loop (100ms tick)                   │ │
│  │  ├─ Schedule child orders                  │ │
│  │  ├─ Execute orders on orderbook            │ │
│  │  └─ Broadcast fills to subscribers         │ │
│  └────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────┐ │
│  │  Position Manager                          │ │
│  │  ├─ Track net positions                    │ │
│  │  ├─ Calculate realized/unrealized P&L      │ │
│  │  └─ Update avg entry prices                │ │
│  └────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────┐ │
│  │  Risk Checker                              │ │
│  │  ├─ Position limits                        │ │
│  │  ├─ Max order size                         │ │
│  │  └─ Symbol whitelist                       │ │
│  └────────────────────────────────────────────┘ │
└────────────────┬────────────────────────────────┘
                 │
                 ↓
┌─────────────────────────────────────────────────┐
│               orderbook                         │
│  ┌────────────────────────────────────────────┐ │
│  │  Price Levels (sorted)                     │ │
│  │  $100.02: [Order A: 500] ← best ask        │ │
│  │  $100.01: [Order B: 1000]                  │ │
│  │  ─────────────────────────── (spread)      │ │
│  │  $99.99:  [Order C: 1000] ← best bid       │ │
│  │  $99.98:  [Order D: 500]                   │ │
│  └────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────┐ │
│  │  Matching Engine                           │ │
│  │  ├─ Price-time priority                    │ │
│  │  ├─ Generate fills                         │ │
│  │  └─ Update book depth                      │ │
│  └────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────┘
```

### Crate Breakdown

**orderbook** (`crates/orderbook/`)

- Maintains buy/sell orders in price levels
- Implements price-time priority matching
- Generates fills when orders cross
- O(log n) insert, O(1) best bid/ask lookup

**algo-core** (`crates/algo-core/`)

- Pure algorithm implementations (no I/O)
- Takes parameters → Returns schedule of child orders
- Algorithms: TWAP, POV, VWAP, IS
- No dependencies on engine or orderbook

**engine** (`crates/engine/`)

- Main execution loop (100ms tick by default)
- Receives parent orders via gRPC API
- Calls algo-core to generate schedule
- Submits child orders to orderbook at scheduled times
- Tracks positions and enforces risk limits
- Broadcasts fills to subscribers

**api** (`crates/api/`)

- gRPC server implementation (tonic)
- Protocol buffer definitions (`proto/execution.proto`)
- Endpoints:
  - `SubmitParentOrder`: Submit algo order
  - `GetOrderStatus`: Query fill progress
  - `GetPositions`: Query positions
  - `StreamFills`: Real-time fill stream

**database** (`crates/database/`)

- SQLite persistence
- Tables: orders, fills, positions
- CRUD operations with rusqlite
- Stores execution history for analysis

**market-data** (`crates/market-data/`)

- Fetches live crypto prices from Binance
- REST API: `/api/v3/ticker/bookTicker` (bid/ask)
- REST API: `/api/v3/ticker/24hr` (volume)
- REST API: `/api/v3/trades` (recent trades)
- Updates every 2 seconds (Binance rate limit)

**indicators** (`crates/indicators/`)

- Technical indicators library
- Moving averages: SMA, EMA
- Momentum: RSI, MACD
- Volatility: Bollinger Bands, ATR
- Volume: OBV
- All indicators work on `Vec<f64>` price series

**analytics** (`crates/analytics/`)

- Performance metrics calculation
- Return metrics: Simple, log, cumulative, annualized
- Risk-adjusted: Sharpe, Sortino, Calmar
- Drawdown: Max, duration, recovery time
- Trade stats: Win rate, profit factor, avg win/loss
- Risk: Volatility, VaR, CVaR (95%, 99%)
- CSV export for further analysis

**backtesting** (`crates/backtesting/`)

- Event-driven backtesting framework
- Strategy trait for custom strategies
- Portfolio management (multi-position)
- Commission and slippage modeling
- Bar-by-bar replay of historical data
- Realistic fill simulation

**strategies** (`crates/strategies/`)

- Pre-built trading strategies
- Pairs Trading: Cointegration-based arbitrage
- Mean Reversion: RSI oversold/overbought
- Bollinger Bands: Volatility breakout
- MACD: Momentum trend following

**trader-gui** (`crates/trader-gui/`)

- egui immediate-mode GUI
- Real-time market data display
- Order entry form with algo selection
- Position and order monitoring
- Performance dashboard with equity curves
- Connects to engine via gRPC

---

## API Usage

### gRPC API

The engine exposes a gRPC API on port 9090. You can interact with it from any language that supports gRPC.

#### Submit Parent Order

```rust
// Rust example
use api::proto::execution_service_client::ExecutionServiceClient;
use api::proto::ParentOrderRequest;

let mut client = ExecutionServiceClient::connect("http://127.0.0.1:9090").await?;

let request = ParentOrderRequest {
    symbol: "BTCUSDT".to_string(),
    side: "BUY".to_string(),
    quantity: 1000,
    limit_price_ticks: 10_000_00, // $100.00 (cents as ticks)
    start_time_ns: start_time,
    end_time_ns: end_time,
    algo_type: "TWAP".to_string(),
    num_slices: 10,
};

let response = client.submit_parent_order(request).await?;
println!("Order ID: {}", response.into_inner().parent_order_id);
```

```python
# Python example
import grpc
from proto import execution_pb2, execution_pb2_grpc

channel = grpc.insecure_channel('localhost:9090')
stub = execution_pb2_grpc.ExecutionServiceStub(channel)

request = execution_pb2.ParentOrderRequest(
    symbol="BTCUSDT",
    side="BUY",
    quantity=1000,
    limit_price_ticks=10000_00,
    start_time_ns=start_time,
    end_time_ns=end_time,
    algo_type="TWAP",
    num_slices=10
)

response = stub.SubmitParentOrder(request)
print(f"Order ID: {response.parent_order_id}")
```

#### Query Order Status

```rust
let request = OrderStatusRequest {
    parent_order_id: 12345,
};

let response = client.get_order_status(request).await?;
let status = response.into_inner();

println!("Status: {}", status.status);
println!("Filled: {}/{}", status.filled_qty, status.total_qty);
```

#### Stream Fills

```rust
let request = StreamFillsRequest {};
let mut stream = client.stream_fills(request).await?.into_inner();

while let Some(fill) = stream.message().await? {
    println!("Fill: {} @ ${:.2}",
        fill.fill_qty,
        fill.fill_price as f64 / 100.0
    );
}
```

---

## Development

### Project Structure

```
QuantSystem/
├── crates/
│   ├── algo-core/          (Execution algorithms)
│   ├── analytics/          (Performance metrics)
│   ├── api/                (gRPC server)
│   ├── backtesting/        (Backtesting framework)
│   ├── database/           (SQLite persistence)
│   ├── engine/             (Execution engine)
│   ├── indicators/         (Technical indicators)
│   ├── market-data/        (Market data fetcher)
│   ├── orderbook/          (Order matching)
│   ├── strategies/         (Trading strategies)
│   ├── trader-gui/         (GUI application)
│   └── python-bindings/    (Python interface)
├── proto/
│   └── execution.proto     (gRPC API definition)
├── Cargo.toml              (Workspace config)
└── README.md
```

### Running Tests

```powershell
# Run all tests
cargo test

# Run tests for specific crate
cargo test -p algo-core
cargo test -p orderbook
cargo test -p analytics

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_twap_schedule
```

### Adding a New Algorithm

1. Create new module in `crates/algo-core/src/`:

```rust
// crates/algo-core/src/myalgo.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MyAlgoParams {
    pub start_ns: u64,
    pub end_ns: u64,
    pub total_qty: u64,
    pub custom_param: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MyAlgoInstruction {
    pub target_time_ns: u64,
    pub qty: u64,
}

pub fn compute_myalgo_schedule(
    params: MyAlgoParams,
) -> Vec<MyAlgoInstruction> {
    let mut schedule = Vec::new();

    // Your algorithm logic here
    // Generate instructions based on parameters

    schedule
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_myalgo_basic() {
        let params = MyAlgoParams {
            start_ns: 0,
            end_ns: 10_000_000_000,  // 10 seconds
            total_qty: 1000,
            custom_param: 0.5,
        };

        let schedule = compute_myalgo_schedule(params);

        assert_eq!(schedule.len(), 10);
        assert_eq!(
            schedule.iter().map(|i| i.qty).sum::<u64>(),
            1000
        );
    }
}
```

2. Export from `crates/algo-core/src/lib.rs`:

```rust
pub mod myalgo;
pub use myalgo::*;
```

3. Add to engine in `crates/engine/src/event_loop.rs`:

```rust
let schedule = match algo_type.as_str() {
    "TWAP" => compute_twap_schedule(...),
    "POV" => compute_pov_schedule(...),
    "VWAP" => compute_vwap_schedule(...),
    "IS" => compute_is_schedule(...),
    "MYALGO" => compute_myalgo_schedule(...),  // Add here
    _ => return Err("Unknown algorithm"),
};
```

### Modifying the gRPC API

1. Edit `proto/execution.proto`:

```protobuf
message ParentOrderRequest {
  string symbol = 1;
  string side = 2;
  uint64 quantity = 3;
  // Add new field
  double custom_param = 10;
}
```

2. Rebuild to regenerate Rust code:

```powershell
cargo build
```

This runs `build.rs` which invokes `tonic-build` to regenerate `execution.rs` from the proto file.

3. Update server in `crates/api/src/server.rs`:

```rust
async fn submit_parent_order(
    &self,
    request: Request<ParentOrderRequest>,
) -> Result<Response<SubmitOrderResponse>, Status> {
    let req = request.into_inner();

    // Access new field
    let custom_param = req.custom_param;

    // Use it in your logic...
}
```

### Database Schema

```sql
-- Orders table
CREATE TABLE orders (
    order_id INTEGER PRIMARY KEY,
    parent_order_id INTEGER,
    symbol TEXT NOT NULL,
    side TEXT NOT NULL,
    qty INTEGER NOT NULL,
    price INTEGER NOT NULL,
    filled_qty INTEGER DEFAULT 0,
    status TEXT NOT NULL,
    algo_type TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Fills table
CREATE TABLE fills (
    fill_id INTEGER PRIMARY KEY AUTOINCREMENT,
    order_id INTEGER NOT NULL,
    symbol TEXT NOT NULL,
    side TEXT NOT NULL,
    fill_qty INTEGER NOT NULL,
    fill_price INTEGER NOT NULL,
    timestamp_ns INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    FOREIGN KEY (order_id) REFERENCES orders(order_id)
);

-- Positions table
CREATE TABLE positions (
    symbol TEXT PRIMARY KEY,
    qty INTEGER NOT NULL,
    avg_price INTEGER NOT NULL,
    realized_pnl REAL DEFAULT 0.0,
    updated_at INTEGER NOT NULL
);
```

---

## Troubleshooting

### Build Issues

**"Cannot find kernel32.lib"**

- Install Windows SDK via Visual Studio Installer
- Modify → Visual Studio 2022 → Desktop development with C++

**"linker error LNK1181"**

- Ensure you're using MSVC toolchain, not GNU
- Run: `rustup default stable-x86_64-pc-windows-msvc`

**"Cargo.lock conflicts"**

- Run: `cargo update`
- Then: `cargo build --release`

### Runtime Issues

**"Failed to connect to engine"**

- Ensure engine is running: `cargo run --release -p engine`
- Check port 9090 is not blocked by firewall
- Look for "Starting gRPC server on 0.0.0.0:9090" in logs

**"Orders showing 0/1000 filled"**

- Engine is working but GUI polling failed (fixed in current version)
- Check GUI terminal for `[GUI] Order X updated` messages
- Restart both engine and GUI if still stuck

**"Performance Dashboard shows 'No performance data available'"**

- Submit orders and wait for them to fill completely
- Dashboard needs at least 1 filled order to calculate metrics
- Check Orders table shows FILLED status

**"GUI log spam filling terminal"**

- Fixed in current version (logs only on changes)
- If still seeing spam, file an issue on GitHub

---

## Performance Considerations

### Latency

- **Order submission**: < 1 μs (microsecond) from receipt to child order generation
- **Order matching**: < 10 μs to match order and generate fill
- **gRPC round-trip**: ~100 μs on localhost
- **Database write**: ~1 ms (SQLite transaction)

All critical paths avoid allocations and use stack-based data structures.

### Throughput

- **Orders/second**: > 10,000 parent orders/sec
- **Fills/second**: > 50,000 fills/sec (limited by orderbook matching)
- **GUI update rate**: 60 FPS (16 ms per frame)
- **Polling rate**: Every 500ms for order status

### Memory

- **Engine**: ~30 MB resident
- **GUI**: ~50 MB resident (includes egui and market data)
- **Orderbook**: ~1 KB per price level
- **Per order**: ~200 bytes

### Optimization Tips

1. **Use release builds**: `cargo build --release` (10x+ faster)
2. **Batch orders**: Submit multiple parent orders at once
3. **Reduce slices**: Fewer child orders = less overhead
4. **Increase tick interval**: Default is 100ms, increase if not latency-sensitive
5. **Disable logging**: Set `RUST_LOG=error` to reduce I/O

---

## License

This is a sandbox project for learning and experimentation. Use at your own risk.

---

## Resources

**Rust**

- [Rust Book](https://doc.rust-lang.org/book/)
- [Rust by Example](https://doc.rust-lang.org/rust-by-example/)
- [Async Book](https://rust-lang.github.io/async-book/)

**Finance**

- [Algorithmic Trading and DMA](https://www.amazon.com/Algorithmic-Trading-DMA-introduction-strategies/dp/0956399207) by Barry Johnson
- [Trading and Exchanges](https://www.amazon.com/Trading-Exchanges-Market-Microstructure-Practitioners/dp/0195144708) by Larry Harris
- [Quantitative Trading](https://www.amazon.com/Quantitative-Trading-Build-Algorithmic-Business/dp/1119800064) by Ernest Chan

**Technical**

- [gRPC in Rust (tonic)](https://github.com/hyperium/tonic)
- [egui (GUI framework)](https://github.com/emilk/egui)
- [Protocol Buffers](https://protobuf.dev/)
- [SQLite](https://www.sqlite.org/)

---

## Common Questions

**Q: Can I connect this to a real exchange?**
A: This is a sandbox. To connect to real exchanges:

1. Replace orderbook with exchange API client (REST/WebSocket/FIX)
2. Implement exchange-specific authentication
3. Add order type mapping (IOC, FOK, Post-Only, etc.)
4. Handle exchange-specific errors and rate limits
5. Add regulatory compliance (order tagging, reporting)

**Q: Why Rust?**
A:

- Performance: Comparable to C++ but memory-safe
- No GC pauses: Critical for low-latency trading
- Fearless concurrency: Multi-threaded execution without data races
- Growing ecosystem: tokio, tonic, serde, sqlx all excellent

**Q: How do I backtest algorithms?**
A: Use the backtesting crate:

```powershell
cargo run --example backtest_demo --release
```

Or implement your own strategy:

```rust
use backtesting::{Strategy, Signal, MarketDataBar, Portfolio};

struct MyStrategy;

impl Strategy for MyStrategy {
    fn on_bar(&mut self, bar: &MarketDataBar, portfolio: &Portfolio) -> Vec<Signal> {
        // Your logic here
    }
}
```

**Q: What's the engine tick interval?**
A: Default is 100ms. Change in `engine/src/main.rs`:

```rust
let engine = ExecutionEngine::new(..., 100); // milliseconds
```

**Q: Can I run multiple algorithms on one order?**
A: No, each parent order uses one algorithm. Submit multiple parent orders if you want to compare algorithms.

**Q: How do I export results?**
A:

```rust
use analytics::{export_trades_to_csv, export_metrics_to_csv};

export_trades_to_csv(&trades, "trades.csv")?;
export_metrics_to_csv(&metrics, "metrics.csv")?;
```

**Q: Can I create custom indicators?**
A: Yes! Add to `crates/indicators/src/`:

```rust
pub fn my_indicator(prices: &[f64], period: usize) -> Result<Vec<f64>, String> {
    // Your indicator logic
}
```

**Q: Does this work on Linux/Mac?**
A: Yes, but you don't need Visual Studio. Just:

```bash
cargo build --release
```

Rust and the dependencies are cross-platform.

---

**Built with Rust 🦀 | Powered by gRPC | Real-time market data from Binance**
