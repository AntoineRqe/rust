use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{query, PgPool, Row};
use std::collections::HashMap;
use utils::market_name;

use super::{PlayerStore, block_on_storage, parse_fix_fields, parse_f64};

/// Summary of a player's holdings in a symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldingSummary {
    pub quantity: f64,
    pub avg_price: f64,
}

/// A single purchased lot in a player's portfolio.
#[derive(Debug, Clone, Serialize)]
pub struct PortfolioLot {
    pub id: i64,
    pub username: String,
    pub symbol: String,
    /// Remaining (un-sold) quantity for this lot.
    pub quantity: f64,
    /// Average purchase price per unit for this lot.
    pub price: f64,
    pub purchased_at: DateTime<Utc>,
}

impl PlayerStore {
    /// Return all open portfolio lots for a player, oldest first.
    ///
    /// Returns an empty vec if the player has no lots or the pool is unavailable.
    pub fn get_portfolio(&self, username: &str) -> Vec<PortfolioLot> {
        let pool = {
            let inner = self.inner.lock().unwrap();
            inner.pool.clone()
        };
        let Some(pool) = pool else {
            return Vec::new();
        };
        block_on_storage(load_portfolio_lots_for_user(&pool, username)).unwrap_or_default()
    }

    /// Return a holdings summary (symbol → total remaining quantity) derived
    /// from open portfolio lots for a player.
    pub fn get_holdings_summary(&self, username: &str) -> HashMap<String, HoldingSummary> {
        let mut total_qty: HashMap<String, f64> = HashMap::new();
        let mut total_cost: HashMap<String, f64> = HashMap::new();

        for lot in self.get_portfolio(username) {
            *total_qty.entry(lot.symbol.clone()).or_insert(0.0) += lot.quantity;
            *total_cost.entry(lot.symbol).or_insert(0.0) += lot.quantity * lot.price;
        }

        total_qty
            .into_iter()
            .filter_map(|(symbol, quantity)| {
                if quantity <= 0.0 {
                    return None;
                }

                let avg_price = total_cost.get(&symbol).copied().unwrap_or(0.0) / quantity;
                Some((
                    symbol,
                    HoldingSummary {
                        quantity,
                        avg_price,
                    },
                ))
            })
            .collect()
    }

    /// Apply a FIX execution report to player state (tokens, pending orders).
    /// Also performs portfolio lot inserts (buy) or FIFO consumes (sell).
    pub fn apply_fix_execution_report(&self, fix_body: &str) -> bool {
        let fields = parse_fix_fields(fix_body);

        if fields.get("35").map(String::as_str) != Some("8") {
            return false;
        }

        let ord_status = fields.get("39").map(String::as_str).unwrap_or("");
        let cl_ord_id = fields
            .get("41")
            .or_else(|| fields.get("11"))
            .map(String::as_str)
            .unwrap_or("");

        if cl_ord_id.is_empty() {
            return false;
        }

        let exec_id = fields.get("17").cloned().unwrap_or_else(|| {
            format!(
                "{}:{}:{}:{}:{}",
                cl_ord_id,
                ord_status,
                fields.get("32").map(String::as_str).unwrap_or("0"),
                fields.get("31").map(String::as_str).unwrap_or("0"),
                fields.get("151").map(String::as_str).unwrap_or("0")
            )
        });

        let last_qty = parse_f64(fields.get("32").map(String::as_str));
        let last_px = parse_f64(fields.get("31").map(String::as_str));
        let leaves_qty = parse_f64(fields.get("151").map(String::as_str));
        let side = fields.get("54").map(String::as_str).unwrap_or("");
        let symbol = fields
            .get("55")
            .map(|s| s.trim().to_uppercase())
            .unwrap_or_default();

        let mut changed = false;
        let mut inner = self.inner.lock().unwrap();

        if !inner.processed_exec_ids.insert(exec_id) {
            return false;
        }

        // Capture owner now — before it may be removed from order_owners below.
        let owner = inner.order_owners.get(cl_ord_id).cloned();

        // Portfolio lot operation to perform after releasing the lock.
        // We determine it here while `owner` is still available.
        let lot_op: Option<(String, String, String, f64, f64)> = if let Some(ref uname) = owner {
            if (ord_status == "1" || ord_status == "2")
                && last_qty > 0.0
                && last_px > 0.0
                && !symbol.is_empty()
                && (side == "1" || side == "2")
            {
                Some((
                    uname.clone(),
                    side.to_string(),
                    symbol.clone(),
                    last_qty,
                    last_px,
                ))
            } else {
                None
            }
        } else {
            None
        };

        if let Some(owner_username) = owner {
            if let Some(player) = inner.players.get_mut(&owner_username) {
                if (ord_status == "1" || ord_status == "2") && last_qty > 0.0 && last_px > 0.0 {
                    let traded_notional = last_qty * last_px;
                    if side == "1" {
                        player.tokens -= traded_notional;
                    } else if side == "2" {
                        player.tokens += traded_notional;
                    }
                    changed = true;
                }

                if let Some(pos) = player
                    .pending_orders
                    .iter()
                    .position(|o| o.cl_ord_id == cl_ord_id)
                {
                    match ord_status {
                        "2" | "3" | "4" | "8" | "C" => {
                            player.pending_orders.remove(pos);
                            changed = true;
                        }
                        "1" => {
                            if leaves_qty > 0.0 {
                                player.pending_orders[pos].qty = leaves_qty;
                                changed = true;
                            } else if last_qty > 0.0 {
                                let current = player.pending_orders[pos].qty;
                                player.pending_orders[pos].qty = (current - last_qty).max(0.0);
                                changed = true;
                            }
                        }
                        _ => {}
                    }
                }
            }

            if matches!(ord_status, "2" | "3" | "4" | "8" | "C") {
                inner.order_owners.remove(cl_ord_id);
                changed = true;
            }
        }

        drop(inner);
        if changed {
            self.flush();
        }

        // Perform portfolio lot insert (buy) or FIFO consume (sell) after flush.
        if let Some((uname, lot_side, lot_symbol, lot_qty, lot_px)) = lot_op {
            let pool = {
                let inner = self.inner.lock().unwrap();
                inner.pool.clone()
            };
            if let Some(pool) = pool {
                if lot_side == "1" {
                    // Buy: insert a new lot.
                    if let Err(e) = block_on_storage(insert_portfolio_lot(
                        &pool,
                        &uname,
                        &lot_symbol,
                        lot_qty,
                        lot_px,
                    )) {
                        tracing::error!(
                            "[{}] Failed to insert portfolio lot for {uname}: {e}",
                            market_name()
                        );
                    }
                } else if lot_side == "2" {
                    // Sell: FIFO consume from existing lots.
                    if let Err(e) = block_on_storage(consume_portfolio_lots_fifo(
                        &pool,
                        &uname,
                        &lot_symbol,
                        lot_qty,
                    )) {
                        tracing::error!(
                            "[{}] Failed to consume portfolio lots for {uname}: {e}",
                            market_name()
                        );
                    }
                }
            }
        }

        changed
    }

    /// Apply the passive side of a trade reconstructed from the multicast
    /// market-data feed. This closes the gap where a resting owner's FIX TCP
    /// connection no longer exists, so their execution report is never
    /// delivered back over the original response queue.
    pub fn apply_trade_from_feed(
        &self,
        trade_id: u64,
        passive_cl_ord_id: &str,
        symbol: &str,
        quantity: f64,
        price: f64,
    ) -> bool {
        let passive_cl_ord_id = passive_cl_ord_id.trim();
        let symbol = symbol.trim().to_uppercase();

        if passive_cl_ord_id.is_empty()
            || symbol.is_empty()
            || !quantity.is_finite()
            || quantity <= 0.0
            || !price.is_finite()
            || price <= 0.0
        {
            return false;
        }

        let exec_id = format!("feed:{trade_id}:{passive_cl_ord_id}");
        let notional = quantity * price;

        let mut changed = false;
        let mut inner = self.inner.lock().unwrap();

        if !inner.processed_exec_ids.insert(exec_id) {
            return false;
        }

        let owner = inner.order_owners.get(passive_cl_ord_id).cloned();

        let lot_op = if let Some(ref owner_username) = owner {
            Some((owner_username.clone(), symbol.clone(), quantity))
        } else {
            None
        };

        if let Some(owner_username) = owner {
            if let Some(player) = inner.players.get_mut(&owner_username) {
                player.tokens += notional;

                if let Some(pos) = player
                    .pending_orders
                    .iter()
                    .position(|o| o.cl_ord_id == passive_cl_ord_id)
                {
                    let current = player.pending_orders[pos].qty;
                    let remaining = (current - quantity).max(0.0);
                    if remaining <= 1e-9 {
                        player.pending_orders.remove(pos);
                        inner.order_owners.remove(passive_cl_ord_id);
                    } else {
                        player.pending_orders[pos].qty = remaining;
                    }
                }

                changed = true;
            }
        }

        drop(inner);
        if changed {
            self.flush();
        }

        if let Some((owner_username, lot_symbol, lot_qty)) = lot_op {
            let pool = {
                let inner = self.inner.lock().unwrap();
                inner.pool.clone()
            };
            if let Some(pool) = pool {
                if let Err(e) = block_on_storage(consume_portfolio_lots_fifo(
                    &pool,
                    &owner_username,
                    &lot_symbol,
                    lot_qty,
                )) {
                    tracing::error!("[{}] Failed to consume passive-side portfolio lots for {owner_username}: {e}", market_name());
                }
            }
        }

        changed
    }
}

// ── Database operations ───────────────────────────────────────────────────────

pub async fn load_portfolio_lots_for_user(
    pool: &PgPool,
    username: &str,
) -> Result<Vec<PortfolioLot>, sqlx::Error> {
    let rows = query(
        "SELECT id, username, symbol, quantity, price, purchased_at \
         FROM portfolio_lots WHERE username = $1 ORDER BY purchased_at ASC",
    )
    .bind(username)
    .fetch_all(pool)
    .await?;

    let mut lots = Vec::with_capacity(rows.len());
    for row in rows {
        lots.push(PortfolioLot {
            id: row.try_get("id")?,
            username: row.try_get("username")?,
            symbol: row.try_get("symbol")?,
            quantity: row.try_get("quantity")?,
            price: row.try_get("price")?,
            purchased_at: row.try_get("purchased_at")?,
        });
    }
    Ok(lots)
}

pub async fn insert_portfolio_lot(
    pool: &PgPool,
    username: &str,
    symbol: &str,
    quantity: f64,
    price: f64,
) -> Result<(), sqlx::Error> {
    query("INSERT INTO portfolio_lots (username, symbol, quantity, price) VALUES ($1, $2, $3, $4)")
        .bind(username)
        .bind(symbol)
        .bind(quantity)
        .bind(price)
        .execute(pool)
        .await?;
    Ok(())
}

/// FIFO consumption: reduce the oldest lots first for the given symbol.
pub async fn consume_portfolio_lots_fifo(
    pool: &PgPool,
    username: &str,
    symbol: &str,
    mut sell_qty: f64,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    let rows = query(
        "SELECT id, quantity FROM portfolio_lots \
         WHERE username = $1 AND symbol = $2 \
         ORDER BY purchased_at ASC \
         FOR UPDATE",
    )
    .bind(username)
    .bind(symbol)
    .fetch_all(&mut *tx)
    .await?;

    for row in rows {
        if sell_qty <= 1e-9 {
            break;
        }
        let lot_id: i64 = row.try_get("id")?;
        let lot_qty: f64 = row.try_get("quantity")?;

        if sell_qty >= lot_qty - 1e-9 {
            // Consume entire lot.
            query("DELETE FROM portfolio_lots WHERE id = $1")
                .bind(lot_id)
                .execute(&mut *tx)
                .await?;
            sell_qty -= lot_qty;
        } else {
            // Partially consume lot.
            query("UPDATE portfolio_lots SET quantity = quantity - $1 WHERE id = $2")
                .bind(sell_qty)
                .bind(lot_id)
                .execute(&mut *tx)
                .await?;
            sell_qty = 0.0;
        }
    }

    tx.commit().await
}

pub async fn delete_all_portfolio_lots(pool: &PgPool) -> Result<(), sqlx::Error> {
    query("DELETE FROM portfolio_lots").execute(pool).await?;
    Ok(())
}
