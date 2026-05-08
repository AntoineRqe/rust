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
    /// from open portfolio lots for a player. Result is cached until the
    /// player's portfolio changes (buy or sell trade).
    pub fn get_holdings_summary(&self, username: &str) -> HashMap<String, HoldingSummary> {
        // Check cache first
        {
            let inner = self.inner.lock().unwrap();
            if let Some(cached) = inner.holdings_cache.get(username) {
                return cached.clone();
            }
        }

        // Cache miss — query DB and compute
        let mut total_qty: HashMap<String, f64> = HashMap::new();
        let mut total_cost: HashMap<String, f64> = HashMap::new();

        for lot in self.get_portfolio(username) {
            *total_qty.entry(lot.symbol.clone()).or_insert(0.0) += lot.quantity;
            *total_cost.entry(lot.symbol).or_insert(0.0) += lot.quantity * lot.price;
        }

        let summary: HashMap<String, HoldingSummary> = total_qty
            .into_iter()
            .filter_map(|(symbol, quantity)| {
                if quantity <= 0.0 {
                    return None;
                }
                let avg_price = total_cost.get(&symbol).copied().unwrap_or(0.0) / quantity;
                Some((symbol, HoldingSummary { quantity, avg_price }))
            })
            .collect();

        // Store in cache
        self.inner
            .lock()
            .unwrap()
            .holdings_cache
            .insert(username.to_string(), summary.clone());

        summary
    }

    /// Invalidate the holdings cache for a player (call after any trade).
    pub fn invalidate_holdings_cache(&self, username: &str) {
        self.inner
            .lock()
            .unwrap()
            .holdings_cache
            .remove(username);
    }

    /// Apply a FIX execution report to player state (tokens, pending orders).
    /// Also performs portfolio lot inserts (buy) or FIFO consumes (sell).
    pub fn apply_fix_execution_report(&self, fix_body: &str) -> Result<(), String> {
        let fields = parse_fix_fields(fix_body);

        let msg_type = fields.get("35").map(String::as_str).unwrap_or("");
        
        // Treat Cancel/Replace Reject (type 9) as a rejection of the cancel operation
        // The original order still exists and should remain in pending
        let is_cancel_reject = msg_type == "9";
        
        // Only accept Execution Reports (type 8) and Cancel Rejects (type 9)
        if msg_type != "8" && msg_type != "9" {
            let err = format!("Invalid message type: expected 8 or 9, got '{}'", msg_type);
            tracing::warn!("apply_fix_execution_report validation failed: {}", err);
            return Err(err);
        }

        let ord_status = fields.get("39").map(String::as_str).unwrap_or("");
        let cl_ord_id = fields
            .get("41")  // Field 41 now contains clOrdId from market data
            .or_else(|| fields.get("11"))  // Fallback to field 11 for backwards compatibility
            .cloned()
            .unwrap_or_default();

            if cl_ord_id.is_empty() {
                let err = format!(
                    "Missing ClOrdID: field 41 and field 11 both empty. Available fields: {}",
                    fields.keys().map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
                );
                tracing::warn!("apply_fix_execution_report validation failed: {}", err);
                return Err(err);
            }

        let mut inner = self.inner.lock().unwrap();

        let last_qty = parse_f64(fields.get("32").map(String::as_str));
        let last_px = parse_f64(fields.get("31").map(String::as_str));
        let leaves_qty = parse_f64(fields.get("151").map(String::as_str));
        let side = fields.get("54").map(String::as_str).unwrap_or("");
        let symbol = fields
            .get("55")
            .map(|s| s.trim().to_uppercase())
            .unwrap_or_default();

        let exec_id = fields
            .get("17")
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "{}:{}:{}:{}:{}",
                    cl_ord_id,
                    ord_status,
                    fields.get("32").map(String::as_str).unwrap_or("0"),
                    fields.get("31").map(String::as_str).unwrap_or("0"),
                    fields.get("151").map(String::as_str).unwrap_or("0")
                )
            });

        let mut changed = false;

        if !inner.processed_exec_ids.insert(exec_id.clone()) {
            drop(inner);
            let msg = format!(
                "Duplicate execution report: exec_id='{}' already processed (cl_ord_id='{}', ord_status='{}', last_qty={}, last_px={})",
                exec_id,
                cl_ord_id,
                ord_status,
                fields.get("32").map(String::as_str).unwrap_or("0"),
                fields.get("31").map(String::as_str).unwrap_or("0")
            );
            tracing::debug!("apply_fix_execution_report: {}", msg);
            // Duplicates are expected on reconnect/replay paths; keep idempotent behavior.
            return Ok(());
        }

        // Capture owner now — before it may be removed from order_owners below.
        let owner = inner.order_owners.get(&cl_ord_id).cloned();
        let found_order = owner.is_some();

        // For admin user, handle orders specially
        if let Some(ref uname) = owner {
            if uname == "admin" {
                // Remove order for successful fills, cancellations, rejections
                if matches!(ord_status, "2" | "3" | "4" | "8" | "C") {
                    inner.order_owners.remove(&cl_ord_id);
                    // Also remove from pending_orders for admin users
                    if let Some(player) = inner.players.get_mut(uname) {
                        if let Some(pos) = player.pending_orders.iter().position(|o| o.cl_ord_id == cl_ord_id) {
                            player.pending_orders.remove(pos);
                        }
                    }
                }
                // Also remove order for cancel rejections (type 9) to free up reserved tokens
                if is_cancel_reject {
                    inner.order_owners.remove(&cl_ord_id);
                    // Also remove from pending_orders for admin users
                    if let Some(player) = inner.players.get_mut(uname) {
                        if let Some(pos) = player.pending_orders.iter().position(|o| o.cl_ord_id == cl_ord_id) {
                            player.pending_orders.remove(pos);
                        }
                    }
                }
                drop(inner);
                self.flush();
                return Ok(());
            }
        }

        // If no owner found (order doesn't exist in our tracking), allow it anyway
        // This can happen with pre-existing orders or orders from other systems
        if !found_order {
            drop(inner);
            return Ok(());
        }

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
                // Token management: deduct on new BUY (status=0), refund on cancel (status=4), 
                // credit on SELL execution (status=1,2). Never deduct on fill—already deducted at status=0.
                match ord_status {
                    "0" => {
                        // Status 0 (New): Deduct tokens for BUY orders to reserve them
                        if side == "1" {
                            let qty = parse_f64(fields.get("38").map(String::as_str));
                            let price = parse_f64(fields.get("44").map(String::as_str).or_else(|| fields.get("6").map(String::as_str)));
                            if qty > 0.0 && price > 0.0 {
                                let notional = qty * price;
                                player.tokens -= notional;
                                changed = true;
                                tracing::debug!(
                                    "[Players] Deducted {:.2} tokens for new BUY order {}, remaining: {:.2}",
                                    notional,
                                    cl_ord_id,
                                    player.tokens
                                );
                            }
                        }
                        // SELL orders don't deduct tokens (they reserve equity instead)
                    }
                    "4" => {
                        // Status 4 (Cancelled): Refund reserved tokens if this was a BUY order
                        if let Some(pos) = player
                            .pending_orders
                            .iter()
                            .position(|o| o.cl_ord_id == cl_ord_id)
                        {
                            let pending_order = &player.pending_orders[pos];
                            if pending_order.side == "1" {
                                let notional = pending_order.qty * pending_order.price;
                                player.tokens += notional;
                                changed = true;
                                tracing::debug!(
                                    "[Players] Refunded {:.2} tokens for cancelled BUY order {}, new balance: {:.2}",
                                    notional,
                                    cl_ord_id,
                                    player.tokens
                                );
                            }
                        }
                    }
                    "1" | "2" => {
                        // Partial fill or complete fill: credit SELL orders only
                        // BUY orders already had tokens deducted at status=0, so no change here
                        if last_qty > 0.0 && last_px > 0.0 && side == "2" {
                            let traded_notional = last_qty * last_px;
                            player.tokens += traded_notional;
                            changed = true;
                            tracing::debug!(
                                "[Players] Credited {:.2} tokens for SELL execution {}, new balance: {:.2}",
                                traded_notional,
                                cl_ord_id,
                                player.tokens
                            );
                        }
                    }
                    _ => {
                        // Other statuses don't affect tokens
                    }
                }

                // Update pending order quantities
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
                        _ => {
                            // For cancel rejections (type 9), also remove from pending to free up tokens
                            if is_cancel_reject {
                                player.pending_orders.remove(pos);
                                changed = true;
                            }
                        }
                    }
                }
            }

            if matches!(ord_status, "2" | "3" | "4" | "8" | "C") {
                inner.order_owners.remove(&cl_ord_id);
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
                let store_inner = std::sync::Arc::clone(&self.inner);
                tokio::spawn(async move {
                    if lot_side == "1" {
                        // Buy: insert a new lot.
                        if let Err(e) = insert_portfolio_lot(&pool, &uname, &lot_symbol, lot_qty, lot_px).await {
                            tracing::error!("[{}] Failed to insert portfolio lot for {uname}: {e}", market_name());
                        } else {
                            store_inner.lock().unwrap().holdings_cache.remove(&uname);
                        }
                    } else if lot_side == "2" {
                        // Sell: FIFO consume from existing lots.
                        if let Err(e) = consume_portfolio_lots_fifo(&pool, &uname, &lot_symbol, lot_qty).await {
                            tracing::error!("[{}] Failed to consume portfolio lots for {uname}: {e}", market_name());
                        } else {
                            store_inner.lock().unwrap().holdings_cache.remove(&uname);
                        }
                    }
                });
            }
        }

    // Return Ok if we found and processed the order (even if no state changed).
    Ok(())
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
                let store_inner = std::sync::Arc::clone(&self.inner);
                tokio::spawn(async move {
                    if let Err(e) = consume_portfolio_lots_fifo(&pool, &owner_username, &lot_symbol, lot_qty).await {
                        tracing::error!("[{}] Failed to consume passive-side portfolio lots for {owner_username}: {e}", market_name());
                    } else {
                        store_inner.lock().unwrap().holdings_cache.remove(&owner_username);
                    }
                });
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
