use chrono::Utc;
use orderbook::{Order, OrderId, Price, Side};
use rusqlite::{params, Connection, Result as SqlResult};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Order not found: {0}")]
    OrderNotFound(String),
}

pub type DatabaseResult<T> = Result<T, DatabaseError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    pub fill_id: i64,
    pub order_id: String,
    pub symbol: String,
    pub side: String,
    pub fill_qty: u64,
    pub fill_price: f64,
    pub timestamp_ns: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub symbol: String,
    pub qty: i64,
    pub avg_price: f64,
    pub realized_pnl: f64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketTick {
    pub tick_id: i64,
    pub symbol: String,
    pub bid_price: f64,
    pub ask_price: f64,
    pub mid_price: f64,
    pub spread: f64,
    pub bid_qty: u64,
    pub ask_qty: u64,
    pub last_price: f64,
    pub volume_24h: f64,
    pub timestamp_ns: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParentOrder {
    pub parent_order_id: String,
    pub symbol: String,
    pub side: String,
    pub total_qty: u64,
    pub filled_qty: u64,
    pub algo_type: String,
    pub algo_params: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildOrder {
    pub child_order_id: String,
    pub parent_order_id: String,
    pub symbol: String,
    pub side: String,
    pub qty: u64,
    pub price: f64,
    pub filled_qty: u64,
    pub status: String,
    pub slice_number: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceSnapshot {
    pub snapshot_id: i64,
    pub timestamp_ns: u64,
    pub total_equity: f64,
    pub cash_balance: f64,
    pub position_value: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub total_pnl: f64,
    pub num_positions: i32,
    pub num_trades: i32,
    pub sharpe_ratio: Option<f64>,
    pub max_drawdown: Option<f64>,
    pub created_at: String,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn new<P: AsRef<Path>>(db_path: P) -> DatabaseResult<Self> {
        let conn = Connection::open(db_path)?;
        let db = Database { conn };
        db.init_schema()?;
        Ok(db)
    }

    pub fn in_memory() -> DatabaseResult<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Database { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> DatabaseResult<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS orders (
                order_id TEXT PRIMARY KEY,
                symbol TEXT NOT NULL,
                side TEXT NOT NULL,
                qty INTEGER NOT NULL,
                price REAL NOT NULL,
                filled_qty INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL,
                algo_type TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS fills (
                fill_id INTEGER PRIMARY KEY AUTOINCREMENT,
                order_id TEXT NOT NULL,
                symbol TEXT NOT NULL,
                side TEXT NOT NULL,
                fill_qty INTEGER NOT NULL,
                fill_price REAL NOT NULL,
                timestamp_ns INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (order_id) REFERENCES orders(order_id)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS positions (
                symbol TEXT PRIMARY KEY,
                qty INTEGER NOT NULL,
                avg_price REAL NOT NULL,
                realized_pnl REAL NOT NULL DEFAULT 0.0,
                updated_at TEXT NOT NULL
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_orders_symbol ON orders(symbol)",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_fills_order_id ON fills(order_id)",
            [],
        )?;

        // New table: market_data_ticks - stores sampled market data
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS market_data_ticks (
                tick_id INTEGER PRIMARY KEY AUTOINCREMENT,
                symbol TEXT NOT NULL,
                bid_price REAL NOT NULL,
                ask_price REAL NOT NULL,
                mid_price REAL NOT NULL,
                spread REAL NOT NULL,
                bid_qty INTEGER NOT NULL,
                ask_qty INTEGER NOT NULL,
                last_price REAL NOT NULL,
                volume_24h REAL NOT NULL,
                timestamp_ns INTEGER NOT NULL,
                created_at TEXT NOT NULL
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_ticks_symbol_time ON market_data_ticks(symbol, timestamp_ns)",
            [],
        )?;

        // New table: parent_orders - tracks parent orders from algorithms
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS parent_orders (
                parent_order_id TEXT PRIMARY KEY,
                symbol TEXT NOT NULL,
                side TEXT NOT NULL,
                total_qty INTEGER NOT NULL,
                filled_qty INTEGER NOT NULL DEFAULT 0,
                algo_type TEXT NOT NULL,
                algo_params TEXT NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_parent_orders_symbol ON parent_orders(symbol)",
            [],
        )?;

        // New table: child_orders - tracks child orders generated by parent orders
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS child_orders (
                child_order_id TEXT PRIMARY KEY,
                parent_order_id TEXT NOT NULL,
                symbol TEXT NOT NULL,
                side TEXT NOT NULL,
                qty INTEGER NOT NULL,
                price REAL NOT NULL,
                filled_qty INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL,
                slice_number INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (parent_order_id) REFERENCES parent_orders(parent_order_id)
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_child_orders_parent ON child_orders(parent_order_id)",
            [],
        )?;

        // New table: performance_snapshots - tracks portfolio performance over time
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS performance_snapshots (
                snapshot_id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp_ns INTEGER NOT NULL,
                total_equity REAL NOT NULL,
                cash_balance REAL NOT NULL,
                position_value REAL NOT NULL,
                unrealized_pnl REAL NOT NULL,
                realized_pnl REAL NOT NULL,
                total_pnl REAL NOT NULL,
                num_positions INTEGER NOT NULL,
                num_trades INTEGER NOT NULL,
                sharpe_ratio REAL,
                max_drawdown REAL,
                created_at TEXT NOT NULL
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_performance_time ON performance_snapshots(timestamp_ns)",
            [],
        )?;

        Ok(())
    }

    pub fn insert_order(
        &self,
        order: &Order,
        algo_type: Option<&str>,
    ) -> DatabaseResult<()> {
        let now = Utc::now().to_rfc3339();
        let side_str = match order.side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };

        self.conn.execute(
            "INSERT INTO orders (order_id, symbol, side, qty, price, filled_qty, status, algo_type, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                order.id.to_string(),
                "UNKNOWN",
                side_str,
                order.qty.value(),
                order.price.as_f64(),
                0,
                "pending",
                algo_type,
                &now,
                &now,
            ],
        )?;

        Ok(())
    }

    pub fn update_order_status(
        &self,
        order_id: &OrderId,
        status: &str,
        filled_qty: u64,
    ) -> DatabaseResult<()> {
        let now = Utc::now().to_rfc3339();

        let rows_affected = self.conn.execute(
            "UPDATE orders SET status = ?1, filled_qty = ?2, updated_at = ?3 WHERE order_id = ?4",
            params![status, filled_qty, &now, order_id.to_string()],
        )?;

        if rows_affected == 0 {
            return Err(DatabaseError::OrderNotFound(order_id.to_string()));
        }

        Ok(())
    }

    pub fn insert_fill(
        &self,
        order_id: &OrderId,
        symbol: &str,
        side: Side,
        fill_qty: u64,
        fill_price: Price,
        timestamp_ns: u64,
    ) -> DatabaseResult<i64> {
        let now = Utc::now().to_rfc3339();
        let side_str = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };

        self.conn.execute(
            "INSERT INTO fills (order_id, symbol, side, fill_qty, fill_price, timestamp_ns, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                order_id.to_string(),
                symbol,
                side_str,
                fill_qty,
                fill_price.as_f64(),
                timestamp_ns,
                &now,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_fills(&self, order_id: Option<&OrderId>) -> DatabaseResult<Vec<Fill>> {
        let query = if order_id.is_some() {
            "SELECT fill_id, order_id, symbol, side, fill_qty, fill_price, timestamp_ns, created_at
             FROM fills WHERE order_id = ?1 ORDER BY timestamp_ns DESC"
        } else {
            "SELECT fill_id, order_id, symbol, side, fill_qty, fill_price, timestamp_ns, created_at
             FROM fills ORDER BY timestamp_ns DESC"
        };

        let mut stmt = self.conn.prepare(query)?;

        let fills = if let Some(oid) = order_id {
            stmt.query_map(params![oid.to_string()], |row| {
                Ok(Fill {
                    fill_id: row.get(0)?,
                    order_id: row.get(1)?,
                    symbol: row.get(2)?,
                    side: row.get(3)?,
                    fill_qty: row.get(4)?,
                    fill_price: row.get(5)?,
                    timestamp_ns: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })?
                .collect::<SqlResult<Vec<_>>>()?
        } else {
            stmt.query_map([], |row| {
                Ok(Fill {
                    fill_id: row.get(0)?,
                    order_id: row.get(1)?,
                    symbol: row.get(2)?,
                    side: row.get(3)?,
                    fill_qty: row.get(4)?,
                    fill_price: row.get(5)?,
                    timestamp_ns: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })?
                .collect::<SqlResult<Vec<_>>>()?
        };

        Ok(fills)
    }

    pub fn upsert_position(
        &self,
        symbol: &str,
        qty_delta: i64,
        price: f64,
    ) -> DatabaseResult<()> {
        let now = Utc::now().to_rfc3339();

        let existing: Option<(i64, f64)> = self.conn
            .query_row(
                "SELECT qty, avg_price FROM positions WHERE symbol = ?1",
                params![symbol],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        match existing {
            Some((current_qty, current_avg_price)) => {
                let new_qty = current_qty + qty_delta;

                let new_avg_price = if new_qty == 0 {
                    0.0
                } else if (current_qty >= 0 && qty_delta >= 0) || (current_qty <= 0 && qty_delta <= 0) {
                    let total_cost = (current_qty as f64 * current_avg_price) + (qty_delta as f64 * price);
                    total_cost / new_qty as f64
                } else {
                    current_avg_price
                };

                self.conn.execute(
                    "UPDATE positions SET qty = ?1, avg_price = ?2, updated_at = ?3 WHERE symbol = ?4",
                    params![new_qty, new_avg_price, &now, symbol],
                )?;
            }
            None => {
                self.conn.execute(
                    "INSERT INTO positions (symbol, qty, avg_price, realized_pnl, updated_at)
                     VALUES (?1, ?2, ?3, 0.0, ?4)",
                    params![symbol, qty_delta, price, &now],
                )?;
            }
        }

        Ok(())
    }

    pub fn get_positions(&self) -> DatabaseResult<Vec<Position>> {
        let mut stmt = self.conn.prepare(
            "SELECT symbol, qty, avg_price, realized_pnl, updated_at FROM positions WHERE qty != 0"
        )?;

        let positions = stmt.query_map([], |row| {
            Ok(Position {
                symbol: row.get(0)?,
                qty: row.get(1)?,
                avg_price: row.get(2)?,
                realized_pnl: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;

        positions.collect::<SqlResult<Vec<_>>>().map_err(Into::into)
    }

    pub fn get_position(&self, symbol: &str) -> DatabaseResult<Option<Position>> {
        let result = self.conn.query_row(
            "SELECT symbol, qty, avg_price, realized_pnl, updated_at FROM positions WHERE symbol = ?1",
            params![symbol],
            |row| {
                Ok(Position {
                    symbol: row.get(0)?,
                    qty: row.get(1)?,
                    avg_price: row.get(2)?,
                    realized_pnl: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        );

        match result {
            Ok(pos) => Ok(Some(pos)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // Market data tick methods
    pub fn insert_market_tick(
        &self,
        symbol: &str,
        bid_price: f64,
        ask_price: f64,
        bid_qty: u64,
        ask_qty: u64,
        last_price: f64,
        volume_24h: f64,
        timestamp_ns: u64,
    ) -> DatabaseResult<i64> {
        let now = Utc::now().to_rfc3339();
        let mid_price = (bid_price + ask_price) / 2.0;
        let spread = ask_price - bid_price;

        self.conn.execute(
            "INSERT INTO market_data_ticks
             (symbol, bid_price, ask_price, mid_price, spread, bid_qty, ask_qty, last_price, volume_24h, timestamp_ns, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                symbol,
                bid_price,
                ask_price,
                mid_price,
                spread,
                bid_qty,
                ask_qty,
                last_price,
                volume_24h,
                timestamp_ns,
                &now,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_market_history(
        &self,
        symbol: &str,
        start_time_ns: Option<u64>,
        end_time_ns: Option<u64>,
        limit: Option<usize>,
    ) -> DatabaseResult<Vec<MarketTick>> {
        let mut query = "SELECT tick_id, symbol, bid_price, ask_price, mid_price, spread, bid_qty, ask_qty, last_price, volume_24h, timestamp_ns, created_at
                         FROM market_data_ticks WHERE symbol = ?1".to_string();

        let mut param_count = 1;
        if start_time_ns.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND timestamp_ns >= ?{}", param_count));
        }
        if end_time_ns.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND timestamp_ns <= ?{}", param_count));
        }

        query.push_str(" ORDER BY timestamp_ns DESC");

        if let Some(lim) = limit {
            query.push_str(&format!(" LIMIT {}", lim));
        }

        let mut stmt = self.conn.prepare(&query)?;

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(symbol.to_string())];
        if let Some(st) = start_time_ns {
            params_vec.push(Box::new(st));
        }
        if let Some(et) = end_time_ns {
            params_vec.push(Box::new(et));
        }

        let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

        let ticks = stmt.query_map(&params_refs[..], |row| {
            Ok(MarketTick {
                tick_id: row.get(0)?,
                symbol: row.get(1)?,
                bid_price: row.get(2)?,
                ask_price: row.get(3)?,
                mid_price: row.get(4)?,
                spread: row.get(5)?,
                bid_qty: row.get(6)?,
                ask_qty: row.get(7)?,
                last_price: row.get(8)?,
                volume_24h: row.get(9)?,
                timestamp_ns: row.get(10)?,
                created_at: row.get(11)?,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;

        Ok(ticks)
    }

    // Parent order methods
    pub fn insert_parent_order(
        &self,
        parent_order_id: &str,
        symbol: &str,
        side: Side,
        total_qty: u64,
        algo_type: &str,
        algo_params: &str,
    ) -> DatabaseResult<()> {
        let now = Utc::now().to_rfc3339();
        let side_str = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };

        self.conn.execute(
            "INSERT INTO parent_orders
             (parent_order_id, symbol, side, total_qty, filled_qty, algo_type, algo_params, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, 'pending', ?7, ?8)",
            params![
                parent_order_id,
                symbol,
                side_str,
                total_qty,
                algo_type,
                algo_params,
                &now,
                &now,
            ],
        )?;

        Ok(())
    }

    pub fn update_parent_order_status(
        &self,
        parent_order_id: &str,
        status: &str,
        filled_qty: u64,
    ) -> DatabaseResult<()> {
        let now = Utc::now().to_rfc3339();

        let rows_affected = self.conn.execute(
            "UPDATE parent_orders SET status = ?1, filled_qty = ?2, updated_at = ?3 WHERE parent_order_id = ?4",
            params![status, filled_qty, &now, parent_order_id],
        )?;

        if rows_affected == 0 {
            return Err(DatabaseError::OrderNotFound(parent_order_id.to_string()));
        }

        Ok(())
    }

    pub fn get_parent_order(&self, parent_order_id: &str) -> DatabaseResult<Option<ParentOrder>> {
        let result = self.conn.query_row(
            "SELECT parent_order_id, symbol, side, total_qty, filled_qty, algo_type, algo_params, status, created_at, updated_at
             FROM parent_orders WHERE parent_order_id = ?1",
            params![parent_order_id],
            |row| {
                Ok(ParentOrder {
                    parent_order_id: row.get(0)?,
                    symbol: row.get(1)?,
                    side: row.get(2)?,
                    total_qty: row.get(3)?,
                    filled_qty: row.get(4)?,
                    algo_type: row.get(5)?,
                    algo_params: row.get(6)?,
                    status: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        );

        match result {
            Ok(order) => Ok(Some(order)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // Child order methods
    pub fn insert_child_order(
        &self,
        child_order_id: &str,
        parent_order_id: &str,
        symbol: &str,
        side: Side,
        qty: u64,
        price: f64,
        slice_number: i32,
    ) -> DatabaseResult<()> {
        let now = Utc::now().to_rfc3339();
        let side_str = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };

        self.conn.execute(
            "INSERT INTO child_orders
             (child_order_id, parent_order_id, symbol, side, qty, price, filled_qty, status, slice_number, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 'pending', ?7, ?8, ?9)",
            params![
                child_order_id,
                parent_order_id,
                symbol,
                side_str,
                qty,
                price,
                slice_number,
                &now,
                &now,
            ],
        )?;

        Ok(())
    }

    pub fn update_child_order_status(
        &self,
        child_order_id: &str,
        status: &str,
        filled_qty: u64,
    ) -> DatabaseResult<()> {
        let now = Utc::now().to_rfc3339();

        let rows_affected = self.conn.execute(
            "UPDATE child_orders SET status = ?1, filled_qty = ?2, updated_at = ?3 WHERE child_order_id = ?4",
            params![status, filled_qty, &now, child_order_id],
        )?;

        if rows_affected == 0 {
            return Err(DatabaseError::OrderNotFound(child_order_id.to_string()));
        }

        Ok(())
    }

    pub fn get_child_orders(&self, parent_order_id: &str) -> DatabaseResult<Vec<ChildOrder>> {
        let mut stmt = self.conn.prepare(
            "SELECT child_order_id, parent_order_id, symbol, side, qty, price, filled_qty, status, slice_number, created_at, updated_at
             FROM child_orders WHERE parent_order_id = ?1 ORDER BY slice_number ASC"
        )?;

        let orders = stmt.query_map(params![parent_order_id], |row| {
            Ok(ChildOrder {
                child_order_id: row.get(0)?,
                parent_order_id: row.get(1)?,
                symbol: row.get(2)?,
                side: row.get(3)?,
                qty: row.get(4)?,
                price: row.get(5)?,
                filled_qty: row.get(6)?,
                status: row.get(7)?,
                slice_number: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })?;

        orders.collect::<SqlResult<Vec<_>>>().map_err(Into::into)
    }

    // Performance snapshot methods
    pub fn insert_performance_snapshot(
        &self,
        timestamp_ns: u64,
        total_equity: f64,
        cash_balance: f64,
        position_value: f64,
        unrealized_pnl: f64,
        realized_pnl: f64,
        num_positions: i32,
        num_trades: i32,
        sharpe_ratio: Option<f64>,
        max_drawdown: Option<f64>,
    ) -> DatabaseResult<i64> {
        let now = Utc::now().to_rfc3339();
        let total_pnl = unrealized_pnl + realized_pnl;

        self.conn.execute(
            "INSERT INTO performance_snapshots
             (timestamp_ns, total_equity, cash_balance, position_value, unrealized_pnl, realized_pnl, total_pnl,
              num_positions, num_trades, sharpe_ratio, max_drawdown, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                timestamp_ns,
                total_equity,
                cash_balance,
                position_value,
                unrealized_pnl,
                realized_pnl,
                total_pnl,
                num_positions,
                num_trades,
                sharpe_ratio,
                max_drawdown,
                &now,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_performance_history(
        &self,
        start_time_ns: Option<u64>,
        end_time_ns: Option<u64>,
        limit: Option<usize>,
    ) -> DatabaseResult<Vec<PerformanceSnapshot>> {
        let mut query = "SELECT snapshot_id, timestamp_ns, total_equity, cash_balance, position_value,
                                unrealized_pnl, realized_pnl, total_pnl, num_positions, num_trades,
                                sharpe_ratio, max_drawdown, created_at
                         FROM performance_snapshots WHERE 1=1".to_string();

        let mut param_count = 0;
        if start_time_ns.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND timestamp_ns >= ?{}", param_count));
        }
        if end_time_ns.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND timestamp_ns <= ?{}", param_count));
        }

        query.push_str(" ORDER BY timestamp_ns DESC");

        if let Some(lim) = limit {
            query.push_str(&format!(" LIMIT {}", lim));
        }

        let mut stmt = self.conn.prepare(&query)?;

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![];
        if let Some(st) = start_time_ns {
            params_vec.push(Box::new(st));
        }
        if let Some(et) = end_time_ns {
            params_vec.push(Box::new(et));
        }

        let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

        let snapshots = stmt.query_map(&params_refs[..], |row| {
            Ok(PerformanceSnapshot {
                snapshot_id: row.get(0)?,
                timestamp_ns: row.get(1)?,
                total_equity: row.get(2)?,
                cash_balance: row.get(3)?,
                position_value: row.get(4)?,
                unrealized_pnl: row.get(5)?,
                realized_pnl: row.get(6)?,
                total_pnl: row.get(7)?,
                num_positions: row.get(8)?,
                num_trades: row.get(9)?,
                sharpe_ratio: row.get(10)?,
                max_drawdown: row.get(11)?,
                created_at: row.get(12)?,
            })
        })?
        .collect::<SqlResult<Vec<_>>>()?;

        Ok(snapshots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_init() {
        let db = Database::in_memory().unwrap();
        assert!(db.get_positions().unwrap().is_empty());
    }

    #[test]
    fn test_position_upsert() {
        let db = Database::in_memory().unwrap();

        db.upsert_position("AAPL", 100, 150.0).unwrap();

        let pos = db.get_position("AAPL").unwrap().unwrap();
        assert_eq!(pos.qty, 100);
        assert_eq!(pos.avg_price, 150.0);

        db.upsert_position("AAPL", 50, 155.0).unwrap();

        let pos = db.get_position("AAPL").unwrap().unwrap();
        assert_eq!(pos.qty, 150);
    }

    #[test]
    fn test_fills_persistence() {
        let db = Database::in_memory().unwrap();

        // First create an order
        let order_id = OrderId::new(12345);
        let order = Order {
            id: order_id.clone(),
            qty: orderbook::Quantity::new(100),
            price: Price::from_f64(150.0),
            side: Side::Buy,
            tif: orderbook::TimeInForce::GTC,
            timestamp: orderbook::Timestamp::new(1000000),
        };
        db.insert_order(&order, Some("TWAP")).unwrap();

        // Now insert a fill
        db.insert_fill(&order_id, "AAPL", Side::Buy, 100, Price::from_f64(150.0), 1000000)
            .unwrap();

        let fills = db.get_fills(Some(&order_id)).unwrap();
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].fill_qty, 100);
    }

    #[test]
    fn test_market_tick_insertion_and_retrieval() {
        let db = Database::in_memory().unwrap();

        // Insert a few market ticks
        db.insert_market_tick("BTCUSDT", 50000.0, 50001.0, 100, 150, 50000.5, 1000000.0, 1000000)
            .unwrap();
        db.insert_market_tick("BTCUSDT", 50100.0, 50101.0, 110, 140, 50100.5, 1100000.0, 2000000)
            .unwrap();
        db.insert_market_tick("ETHUSDT", 3000.0, 3001.0, 200, 250, 3000.5, 500000.0, 1500000)
            .unwrap();

        // Retrieve ticks for BTCUSDT
        let ticks = db.get_market_history("BTCUSDT", None, None, None).unwrap();
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0].bid_price, 50100.0);
        assert_eq!(ticks[0].ask_price, 50101.0);
        assert_eq!(ticks[0].spread, 1.0);

        // Retrieve with time filter
        let ticks_filtered = db
            .get_market_history("BTCUSDT", Some(1500000), None, None)
            .unwrap();
        assert_eq!(ticks_filtered.len(), 1);

        // Retrieve with limit
        let ticks_limited = db.get_market_history("BTCUSDT", None, None, Some(1)).unwrap();
        assert_eq!(ticks_limited.len(), 1);
    }

    #[test]
    fn test_parent_order_lifecycle() {
        let db = Database::in_memory().unwrap();

        // Insert parent order
        db.insert_parent_order(
            "parent_123",
            "BTCUSDT",
            Side::Buy,
            1000,
            "TWAP",
            r#"{"duration_secs": 300, "slice_secs": 60}"#,
        )
        .unwrap();

        // Retrieve parent order
        let parent = db.get_parent_order("parent_123").unwrap().unwrap();
        assert_eq!(parent.parent_order_id, "parent_123");
        assert_eq!(parent.symbol, "BTCUSDT");
        assert_eq!(parent.total_qty, 1000);
        assert_eq!(parent.filled_qty, 0);
        assert_eq!(parent.status, "pending");
        assert_eq!(parent.algo_type, "TWAP");

        // Update parent order status
        db.update_parent_order_status("parent_123", "partial", 500)
            .unwrap();

        let parent_updated = db.get_parent_order("parent_123").unwrap().unwrap();
        assert_eq!(parent_updated.filled_qty, 500);
        assert_eq!(parent_updated.status, "partial");

        // Non-existent parent order
        let result = db.get_parent_order("non_existent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_child_order_tracking() {
        let db = Database::in_memory().unwrap();

        // First insert parent order
        db.insert_parent_order("parent_456", "ETHUSDT", Side::Sell, 500, "VWAP", "{}").unwrap();

        // Insert multiple child orders
        db.insert_child_order("child_1", "parent_456", "ETHUSDT", Side::Sell, 100, 3000.0, 1)
            .unwrap();
        db.insert_child_order("child_2", "parent_456", "ETHUSDT", Side::Sell, 150, 3001.0, 2)
            .unwrap();
        db.insert_child_order("child_3", "parent_456", "ETHUSDT", Side::Sell, 250, 3002.0, 3)
            .unwrap();

        // Retrieve child orders
        let children = db.get_child_orders("parent_456").unwrap();
        assert_eq!(children.len(), 3);
        assert_eq!(children[0].slice_number, 1);
        assert_eq!(children[1].slice_number, 2);
        assert_eq!(children[2].slice_number, 3);
        assert_eq!(children[0].qty, 100);
        assert_eq!(children[1].qty, 150);
        assert_eq!(children[2].qty, 250);

        // Update child order status
        db.update_child_order_status("child_1", "filled", 100)
            .unwrap();

        // Verify update
        let children_updated = db.get_child_orders("parent_456").unwrap();
        assert_eq!(children_updated[0].status, "filled");
        assert_eq!(children_updated[0].filled_qty, 100);
    }

    #[test]
    fn test_performance_snapshot_tracking() {
        let db = Database::in_memory().unwrap();

        // Insert performance snapshots
        db.insert_performance_snapshot(
            1000000,
            100000.0,
            50000.0,
            50000.0,
            2000.0,
            1000.0,
            5,
            10,
            Some(1.5),
            Some(0.05),
        )
        .unwrap();

        db.insert_performance_snapshot(
            2000000,
            105000.0,
            52000.0,
            53000.0,
            3000.0,
            2000.0,
            6,
            12,
            Some(1.6),
            Some(0.04),
        )
        .unwrap();

        // Retrieve all snapshots
        let snapshots = db.get_performance_history(None, None, None).unwrap();
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].total_equity, 105000.0);
        assert_eq!(snapshots[0].total_pnl, 5000.0);

        // Retrieve with time filter
        let snapshots_filtered = db
            .get_performance_history(Some(1500000), None, None)
            .unwrap();
        assert_eq!(snapshots_filtered.len(), 1);
        assert_eq!(snapshots_filtered[0].timestamp_ns, 2000000);

        // Retrieve with limit
        let snapshots_limited = db.get_performance_history(None, None, Some(1)).unwrap();
        assert_eq!(snapshots_limited.len(), 1);
    }

    #[test]
    fn test_schema_migration() {
        // Test that all tables are created correctly
        let db = Database::in_memory().unwrap();

        // Verify market_data_ticks table exists
        let result: Result<i32, _> = db.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='market_data_ticks'",
            [],
            |row| row.get(0),
        );
        assert_eq!(result.unwrap(), 1);

        // Verify parent_orders table exists
        let result: Result<i32, _> = db.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='parent_orders'",
            [],
            |row| row.get(0),
        );
        assert_eq!(result.unwrap(), 1);

        // Verify child_orders table exists
        let result: Result<i32, _> = db.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='child_orders'",
            [],
            |row| row.get(0),
        );
        assert_eq!(result.unwrap(), 1);

        // Verify performance_snapshots table exists
        let result: Result<i32, _> = db.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='performance_snapshots'",
            [],
            |row| row.get(0),
        );
        assert_eq!(result.unwrap(), 1);
    }
}
