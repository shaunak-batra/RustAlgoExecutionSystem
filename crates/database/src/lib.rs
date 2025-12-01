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

        let order_id = OrderId::new(12345);
        db.insert_fill(&order_id, "AAPL", Side::Buy, 100, Price::from_f64(150.0), 1000000)
            .unwrap();

        let fills = db.get_fills(Some(&order_id)).unwrap();
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].fill_qty, 100);
    }
}
