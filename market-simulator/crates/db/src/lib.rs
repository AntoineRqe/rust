use dotenvy::dotenv;
use sqlx::{query, query_scalar, PgPool, Row, Transaction, Postgres};
use sqlx::postgres::PgPoolOptions;
use std::env;
use std::future::Future;
use std::time::Instant;
use std::time::Duration;
use std::sync::OnceLock;
use types::{
    FixedPointArithmetic,
    OrderEvent,
    OrderType,
    OrderResult,
    OrderStatus,
    Side,
    Trade,
};
use types::macros::{EntityId, SymbolId, OrderId};
use spsc::spsc_lock_free::{Consumer};
use std::sync::{Arc, atomic::{AtomicBool}};
use std::sync::atomic::Ordering;

fn market_name() -> &'static str {
    static MARKET_NAME: OnceLock<String> = OnceLock::new();
    MARKET_NAME
        .get_or_init(|| env::var("MARKET_NAME").unwrap_or_else(|_| "unknown".to_string()))
        .as_str()
}

pub struct DatabaseEngine<'a, const N: usize> {
    fifo_in: Consumer<'a, (OrderEvent, OrderResult), N>,
    shutdown: Arc<AtomicBool>,
    pool: Arc<PgPool>,
}

impl <'a, const N: usize> DatabaseEngine<'a, N> {
    pub fn new(
        fifo_in: Consumer<'a, (OrderEvent, OrderResult), N>,
        database_url: &str,
    ) -> Result<Self, sqlx::Error> {
        let pool = Arc::new(block_on_db(connect(database_url))?);

        Ok(Self {
            fifo_in,
            shutdown: Arc::new(AtomicBool::new(false)),
            pool,
        })
    }

    pub fn init(&self) -> Result<(), sqlx::Error> {
        block_on_db(create_tables(&self.pool))
    }

    pub fn run(&self) {
        while !self.shutdown.load(Ordering::Relaxed) || !self.fifo_in.is_empty() {
            if let Some(exec_report) = self.fifo_in.pop() {
                if exec_report.0.sender_id == EntityId::from_ascii("") {
                    self.shutdown.store(true, Ordering::Relaxed);
                } else {
                    if let Err(e) = self.persist_order_update(&exec_report.0, &exec_report.1) {
                        tracing::error!("[{}] Error persisting order update: {}", market_name(), e);
                    }
                }
            }
        }
        
        block_on_db(self.pool.close());

        tracing::info!("[{}] Database engine shutting down gracefully", market_name());
    }

    pub fn persist_order_update(
        &self,
        order_event: &OrderEvent,
        order_result: &OrderResult,
    ) -> Result<(), sqlx::Error> {
        block_on_db(persist_order_update(&self.pool, order_event, order_result))
    }

    pub fn get_all_order_events(&self) -> Result<Vec<OrderEvent>, sqlx::Error> {
        block_on_db(collect_all_orders(&self.pool))
    }

    pub fn get_all_order_results(&self) -> Result<Vec<OrderResult>, sqlx::Error> {
        block_on_db(collect_all_order_results(&self.pool))
    }

    pub fn get_all_pending_orders(&self) -> Result<Vec<OrderEvent>, sqlx::Error> {
        block_on_db(collect_all_pending_orders(&self.pool))
    }

    pub fn reset_database(&self) -> Result<(), sqlx::Error> {
        block_on_db(reset_database(&self.pool))
    }

    /// Returns a clone of the shared database pool, suitable for passing to
    /// the gRPC control service.
    pub fn pool(&self) -> Arc<PgPool> {
        Arc::clone(&self.pool)
    }
}

fn block_on_db<F>(future: F) -> F::Output
where
    F: Future,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(future))
    } else {
        static DB_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        let runtime = DB_RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("db-runtime")
                .build()
                .expect("failed to build shared tokio runtime for database operations")
        });
        runtime.block_on(future)
    }
}

pub async fn connect_from_env() -> Result<PgPool, sqlx::Error> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    connect(&database_url).await
}

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    tracing::debug!("[{}] Connecting to database at {}", market_name(), database_url);
    let max_connections = env::var("DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(10);
    let acquire_timeout_secs = env::var("DB_ACQUIRE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10);

    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(acquire_timeout_secs))
        .connect(database_url)
        .await
}

pub async fn create_tables(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    query("SELECT pg_advisory_xact_lock($1)")
        .bind(42_4242_i64)
        .execute(&mut *tx)
        .await?;

    let order_event_exists: Option<String> = query_scalar("SELECT to_regclass($1)::text")
        .bind("public.order_event")
        .fetch_one(&mut *tx)
        .await?;

    if order_event_exists.is_none() {
        query(
            r#"
            CREATE TABLE IF NOT EXISTS order_event (
                id BIGSERIAL PRIMARY KEY,
                price DOUBLE PRECISION,
                quantity DOUBLE PRECISION,
                side TEXT,
                symbol TEXT,
                order_type TEXT,
                cl_ord_id TEXT,
                orig_cl_ord_id TEXT,
                order_id TEXT,
                sender_id TEXT,
                target_id TEXT,
                event_timestamp TEXT,
                payload JSONB,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&mut *tx)
        .await?;
    }

    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS price DOUBLE PRECISION")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS quantity DOUBLE PRECISION")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS side TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS symbol TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS order_type TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS cl_ord_id TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS orig_cl_ord_id TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS order_id TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS sender_id TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS target_id TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS event_timestamp TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS payload JSONB")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE order_event ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()")
        .execute(&mut *tx)
        .await?;

    let order_result_exists: Option<String> = query_scalar("SELECT to_regclass($1)::text")
        .bind("public.order_result")
        .fetch_one(&mut *tx)
        .await?;

    if order_result_exists.is_none() {
        query(
            r#"
            CREATE TABLE IF NOT EXISTS order_result (
                id BIGSERIAL PRIMARY KEY,
                cl_ord_id TEXT,
                internal_order_id BIGINT,
                result_type TEXT NOT NULL,
                status TEXT,
                reason TEXT,
                payload JSONB,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&mut *tx)
        .await?;
    }

    query("ALTER TABLE order_result ADD COLUMN IF NOT EXISTS internal_order_id BIGINT")
        .execute(&mut *tx)
        .await?;

    let trades_exists: Option<String> = query_scalar("SELECT to_regclass($1)::text")
        .bind("public.trades")
        .fetch_one(&mut *tx)
        .await?;
    if trades_exists.is_none() {
        query(
            r#"
            CREATE TABLE IF NOT EXISTS trades (
                id BIGSERIAL PRIMARY KEY,
                trade_id BIGINT,
                exec_id TEXT UNIQUE,
                symbol TEXT,
                buy_cl_ord_id TEXT,
                sell_cl_ord_id TEXT,
                qty DOUBLE PRECISION NOT NULL,
                price DOUBLE PRECISION NOT NULL,
                payload JSONB,
                traded_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&mut *tx)
        .await?;
    }

    query("ALTER TABLE trades ADD COLUMN IF NOT EXISTS trade_id BIGINT")
        .execute(&mut *tx)
        .await?;

    let pending_orders_exists: Option<String> = query_scalar("SELECT to_regclass($1)::text")
        .bind("public.pending_orders")
        .fetch_one(&mut *tx)
        .await?;
    if pending_orders_exists.is_none() {
        query(
            r#"
            CREATE TABLE IF NOT EXISTS pending_orders (
                cl_ord_id TEXT PRIMARY KEY,
                price DOUBLE PRECISION,
                quantity DOUBLE PRECISION,
                side TEXT,
                symbol TEXT,
                order_type TEXT,
                orig_cl_ord_id TEXT,
                order_id TEXT,
                sender_id TEXT,
                target_id TEXT,
                event_timestamp TEXT,
                payload JSONB,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&mut *tx)
        .await?;
    }

    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS price DOUBLE PRECISION")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS quantity DOUBLE PRECISION")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS side TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS symbol TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS order_type TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS orig_cl_ord_id TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS order_id TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS sender_id TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS target_id TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS event_timestamp TEXT")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS payload JSONB")
        .execute(&mut *tx)
        .await?;
    query("ALTER TABLE pending_orders ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()")
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(())
}

pub async fn reset_database(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    query("SELECT pg_advisory_xact_lock($1)")
        .bind(42_4242_i64)
        .execute(&mut *tx)
        .await?;

    query("TRUNCATE TABLE trades, order_result, order_event RESTART IDENTITY")
        .execute(&mut *tx)
        .await?;

    query("TRUNCATE TABLE pending_orders")
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

pub async fn collect_all_orders(pool: &PgPool) -> Result<Vec<OrderEvent>, sqlx::Error> {
    let rows = query(
        r#"
        SELECT
            price,
            quantity,
            side,
            symbol,
            order_type,
            cl_ord_id,
            orig_cl_ord_id,
            order_id,
            sender_id,
            target_id
        FROM order_event
        ORDER BY id ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| OrderEvent {
            price: FixedPointArithmetic::from_option_f64(row.get("price")),
            quantity: FixedPointArithmetic::from_option_f64(row.get("quantity")),
            side: parse_side(row.get("side")),
            symbol: symbol_id_from_option(row.get("symbol")),
            order_type: parse_order_type(row.get("order_type")),
            cl_ord_id: order_id_from_option(row.get("cl_ord_id")),
            orig_cl_ord_id: row
                .get::<Option<String>, _>("orig_cl_ord_id")
                .map(|value| OrderId::from_ascii(&value)),
            sender_id: entity_id_from_option(row.get("sender_id")),
            target_id: entity_id_from_option(row.get("target_id")),
            // `Instant` cannot be faithfully reconstructed from SQL text, so we
            // restore a fresh monotonic timestamp here.
            timestamp: Instant::now(),
        })
        .collect())
}

pub async fn collect_all_trades(pool: &PgPool) -> Result<Vec<Trade>, sqlx::Error> {
    let rows = query(
        r#"
        SELECT
            trade_id,
            exec_id,
            buy_cl_ord_id,
            sell_cl_ord_id,
            qty,
            price,
            (payload->>'order_qty')::double precision AS order_qty,
            (payload->>'leaves_qty')::double precision AS leaves_qty
        FROM trades
        ORDER BY id ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| Trade {
            price: FixedPointArithmetic::from_option_f64(row.get("price")),
            quantity: FixedPointArithmetic::from_option_f64(row.get("qty")),
            id: row
                .get::<Option<i64>, _>("trade_id")
                .map(|value| value as u64)
                .or_else(|| trade_id_from_option(row.get("exec_id")))
                .unwrap_or_default(),
            cl_ord_id: row
                .get::<Option<String>, _>("sell_cl_ord_id")
                .map(|value| OrderId::from_ascii(&value))
                .or_else(|| {
                    row.get::<Option<String>, _>("buy_cl_ord_id")
                        .map(|value| OrderId::from_ascii(&value))
                })
                .unwrap_or_default(),
            order_qty: FixedPointArithmetic::from_option_f64(row.get("order_qty")),
            leaves_qty: FixedPointArithmetic::from_option_f64(row.get("leaves_qty")),
            // `Instant` cannot be faithfully reconstructed from SQL text, so we
            // restore a fresh monotonic timestamp here.
            timestamp: Instant::now(),
        })
        .collect())
}

pub async fn collect_all_order_results(pool: &PgPool) -> Result<Vec<OrderResult>, sqlx::Error> {
    let rows = query(
        r#"
        SELECT
            internal_order_id,
            status
        FROM order_result
        ORDER BY id ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| OrderResult {
            internal_order_id: row
                .get::<Option<i64>, _>("internal_order_id")
                .unwrap_or_default() as u64,
            trades: types::Trades::<4>::new(),
            status: parse_order_status(row.get("status")),
            // `Instant` cannot be faithfully reconstructed from SQL text, so we
            // restore a fresh monotonic timestamp here.
            timestamp: Instant::now(),
        })
        .collect())
}

pub async fn collect_all_pending_orders(pool: &PgPool) -> Result<Vec<OrderEvent>, sqlx::Error> {
    let rows = query(
        r#"
        SELECT
            price,
            quantity,
            side,
            symbol,
            order_type,
            cl_ord_id,
            orig_cl_ord_id,
            order_id,
            sender_id,
            target_id
        FROM pending_orders
        ORDER BY cl_ord_id ASC
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| OrderEvent {
            price: FixedPointArithmetic::from_option_f64(row.get("price")),
            quantity: FixedPointArithmetic::from_option_f64(row.get("quantity")),
            side: parse_side(row.get("side")),
            symbol: symbol_id_from_option(row.get("symbol")),
            order_type: parse_order_type(row.get("order_type")),
            cl_ord_id: order_id_from_option(row.get("cl_ord_id")),
            orig_cl_ord_id: row
                .get::<Option<String>, _>("orig_cl_ord_id")
                .map(|value| OrderId::from_ascii(&value)),
            sender_id: entity_id_from_option(row.get("sender_id")),
            target_id: entity_id_from_option(row.get("target_id")),
            timestamp: Instant::now(),
        })
        .collect())
}

pub async fn persist_order_update(
    pool: &PgPool,
    order_event: &OrderEvent,
    order_result: &OrderResult,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    let result_type = classify_result_type(order_event, order_result);
    let status = format!("{:?}", order_result.status);
    let reason = match order_result.status {
        OrderStatus::CancelRejected => Some("cancel_rejected"),
        OrderStatus::Cancelled => Some("cancelled"),
        _ => None,
    };

    query(
        r#"
        INSERT INTO order_event (
            price,
            quantity,
            side,
            symbol,
            order_type,
            cl_ord_id,
            orig_cl_ord_id,
            sender_id,
            target_id,
            event_timestamp,
            payload
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, to_jsonb($11::text))
        "#,
    )
    .bind(order_event.price.to_f64())
    .bind(order_event.quantity.to_f64())
    .bind(format!("{:?}", order_event.side))
    .bind(order_event.symbol.to_string())
    .bind(format!("{:?}", order_event.order_type))
    .bind(order_event.cl_ord_id.to_string())
    .bind(order_event.orig_cl_ord_id.map(|id| id.to_string()))
    .bind(order_event.sender_id.to_string())
    .bind(order_event.target_id.to_string())
    .bind(format!("{:?}", order_event.timestamp))
    .bind(format!("{}", order_event))
    .execute(&mut *tx)
    .await?;

    query(
        r#"
        INSERT INTO order_result (
            cl_ord_id,
            internal_order_id,
            result_type,
            status,
            reason,
            payload
        )
        VALUES ($1, $2, $3, $4, $5, to_jsonb($6::text))
        "#,
    )
    .bind(order_event.cl_ord_id.to_string())
    .bind(order_result.internal_order_id as i64)
    .bind(result_type)
    .bind(status)
    .bind(reason)
    .bind(format!("{}", order_result))
    .execute(&mut *tx)
    .await?;

    if matches!(result_type, "FILL" | "PARTIAL_FILL") {
        for trade in order_result.trades.iter() {
            let (buy_cl_ord_id, sell_cl_ord_id) = match order_event.side {
                Side::Buy => (
                    Some(order_event.cl_ord_id.to_string()),
                    Some(trade.cl_ord_id.to_string()),
                ),
                Side::Sell => (
                    Some(trade.cl_ord_id.to_string()),
                    Some(order_event.cl_ord_id.to_string()),
                ),
            };

            query(
                r#"
                INSERT INTO trades (
                    trade_id,
                    exec_id,
                    symbol,
                    buy_cl_ord_id,
                    sell_cl_ord_id,
                    qty,
                    price,
                    payload
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, jsonb_build_object('order_qty', $8, 'leaves_qty', $9))
                ON CONFLICT (exec_id) DO NOTHING
                "#,
            )
            .bind(trade.id as i64)
            .bind(trade.id.to_string())
            .bind(order_event.symbol.to_string())
            .bind(buy_cl_ord_id)
            .bind(sell_cl_ord_id)
            .bind(trade.quantity.to_f64())
            .bind(trade.price.to_f64())
            .bind(trade.order_qty.to_f64())
            .bind(trade.leaves_qty.to_f64())
            .execute(&mut *tx)
            .await?;
        }
    }

    sync_pending_orders(&mut tx, order_event, order_result, result_type).await?;

    tx.commit().await?;
    Ok(())
}

async fn sync_pending_orders(
    tx: &mut Transaction<'_, Postgres>,
    order_event: &OrderEvent,
    order_result: &OrderResult,
    result_type: &str,
) -> Result<(), sqlx::Error> {
    match order_result.status {
        OrderStatus::Cancelled => {
            let target_id = order_event
                .orig_cl_ord_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| order_event.cl_ord_id.to_string());
            query("DELETE FROM pending_orders WHERE cl_ord_id = $1")
                .bind(target_id)
                .execute(&mut **tx)
                .await?;
            return Ok(());
        }
        OrderStatus::CancelRejected => return Ok(()),
        _ => {}
    }

    if order_event.order_type == OrderType::CancelOrder {
        return Ok(());
    }

    let remaining_qty = remaining_quantity(order_event, order_result);

    match result_type {
        "NEW" | "PARTIAL_FILL" if remaining_qty > FixedPointArithmetic::ZERO => {
            query(
                r#"
                INSERT INTO pending_orders (
                    cl_ord_id,
                    price,
                    quantity,
                    side,
                    symbol,
                    order_type,
                    orig_cl_ord_id,
                    sender_id,
                    target_id,
                    event_timestamp,
                    payload,
                    updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, to_jsonb($11::text), NOW())
                ON CONFLICT (cl_ord_id) DO UPDATE SET
                    price = EXCLUDED.price,
                    quantity = EXCLUDED.quantity,
                    side = EXCLUDED.side,
                    symbol = EXCLUDED.symbol,
                    order_type = EXCLUDED.order_type,
                    orig_cl_ord_id = EXCLUDED.orig_cl_ord_id,
                    sender_id = EXCLUDED.sender_id,
                    target_id = EXCLUDED.target_id,
                    event_timestamp = EXCLUDED.event_timestamp,
                    payload = EXCLUDED.payload,
                    updated_at = NOW()
                "#,
            )
            .bind(order_event.cl_ord_id.to_string())
            .bind(order_event.price.to_f64())
            .bind(remaining_qty.to_f64())
            .bind(format!("{:?}", order_event.side))
            .bind(order_event.symbol.to_string())
            .bind(format!("{:?}", order_event.order_type))
            .bind(order_event.orig_cl_ord_id.map(|id| id.to_string()))
            .bind(order_event.sender_id.to_string())
            .bind(order_event.target_id.to_string())
            .bind(format!("{:?}", order_event.timestamp))
            .bind(format!("{}", order_event))
            .execute(&mut **tx)
            .await?;
        }
        _ => {
            query("DELETE FROM pending_orders WHERE cl_ord_id = $1")
                .bind(order_event.cl_ord_id.to_string())
                .execute(&mut **tx)
                .await?;
        }
    }

    Ok(())
}

fn remaining_quantity(order_event: &OrderEvent, order_result: &OrderResult) -> FixedPointArithmetic {
    let traded_qty = order_result.trades.quantity_sum();
    if traded_qty >= order_event.quantity {
        FixedPointArithmetic::ZERO
    } else {
        order_event.quantity - traded_qty
    }
}

fn classify_result_type(order_event: &OrderEvent, order_result: &OrderResult) -> &'static str {
    match order_result.status {
        OrderStatus::Filled => "FILL",
        OrderStatus::PartiallyFilled => "PARTIAL_FILL",
        OrderStatus::Cancelled => "CANCELLED",
        OrderStatus::CancelRejected => "CANCEL_REJECTED",
        OrderStatus::New => {
            let traded_qty = order_result.trades.quantity_sum();
            if traded_qty == FixedPointArithmetic::ZERO {
                "NEW"
            } else if traded_qty >= order_event.quantity {
                "FILL"
            } else {
                "PARTIAL_FILL"
            }
        }
    }
}

fn parse_order_status(value: Option<String>) -> OrderStatus {
    match value.as_deref() {
        Some("Filled") => OrderStatus::Filled,
        Some("PartiallyFilled") => OrderStatus::PartiallyFilled,
        Some("Cancelled") => OrderStatus::Cancelled,
        Some("CancelRejected") => OrderStatus::CancelRejected,
        _ => OrderStatus::New,
    }
}

fn parse_side(value: Option<String>) -> Side {
    match value.as_deref() {
        Some("Sell") => Side::Sell,
        _ => Side::Buy,
    }
}

fn parse_order_type(value: Option<String>) -> OrderType {
    match value.as_deref() {
        Some("MarketOrder") => OrderType::MarketOrder,
        Some("CancelOrder") => OrderType::CancelOrder,
        _ => OrderType::LimitOrder,
    }
}

fn symbol_id_from_option(value: Option<String>) -> SymbolId {
    value
        .map(|value| SymbolId::from_ascii(&value))
        .unwrap_or_default()
}

fn order_id_from_option(value: Option<String>) -> OrderId {
    value
        .map(|value| OrderId::from_ascii(&value))
        .unwrap_or_default()
}

fn entity_id_from_option(value: Option<String>) -> EntityId {
    value
        .map(|value| EntityId::from_ascii(&value))
        .unwrap_or_default()
}

fn trade_id_from_option(value: Option<String>) -> Option<u64> {
    value
    .and_then(|raw| raw.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use std::time::Instant;
    use tokio::sync::Mutex;
    use types::{OrderType, Trade, Trades};
    use types::macros::{EntityId, OrderId};

    static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

    #[tokio::test]
    async fn test_db_connection() -> Result<(), sqlx::Error> {
        let _guard = TEST_MUTEX.get_or_init(|| Mutex::new(())).lock().await;

        let pool = connect_from_env().await?;
        create_tables(&pool).await?;
        reset_database(&pool).await?;

        let orders = collect_all_orders(&pool).await?;
        let trades = collect_all_trades(&pool).await?;

        assert!(orders.is_empty());
        assert!(trades.is_empty());

        reset_database(&pool).await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_persist_order_update() -> Result<(), sqlx::Error> {
        let _guard = TEST_MUTEX.get_or_init(|| Mutex::new(())).lock().await;

        let pool = connect_from_env().await?;
        create_tables(&pool).await?;

        reset_database(&pool).await?;

        let order_event = OrderEvent {
            price: FixedPointArithmetic::from_f64(10.0),
            quantity: FixedPointArithmetic::from_f64(100.0),
            side: Side::Buy,
            symbol: SymbolId::from_ascii("TEST"),
            order_type: OrderType::LimitOrder,
            cl_ord_id: OrderId::from_ascii("test123"),
            orig_cl_ord_id: Some(OrderId::from_ascii("orig123")),
            sender_id: EntityId::from_ascii("broker-a"),
            target_id: EntityId::from_ascii("broker-b"),
            timestamp: Instant::now(),
        };

        let mut trades = Trades::<4>::new();
        trades.add_trade(Trade {
            price: FixedPointArithmetic::from_f64(10.0),
            quantity: FixedPointArithmetic::from_f64(100.0),
            id: 123,
            cl_ord_id: OrderId::from_ascii("maker123"),
            order_qty: FixedPointArithmetic::from_f64(100.0),
            leaves_qty: FixedPointArithmetic::ZERO,
            timestamp: Instant::now(),
        }).unwrap();

        let order_result = OrderResult {
            internal_order_id: 101,
            status: OrderStatus::Filled,
            trades,
            timestamp: Instant::now(),
        };

        persist_order_update(&pool, &order_event, &order_result).await?;
        let orders = collect_all_orders(&pool).await?;
        let trades = collect_all_trades(&pool).await?;

        assert_eq!(orders.len(), 1);
        assert_eq!(trades.len(), 1);
        assert_eq!(orders[0].cl_ord_id.to_string(), "test123");
        assert_eq!(orders[0].orig_cl_ord_id.map(|id| id.to_string()).as_deref(), Some("orig123"));
        assert_eq!(orders[0].sender_id.to_string(), "broker-a");
        assert_eq!(orders[0].target_id.to_string(), "broker-b");
        assert_eq!(orders[0].symbol.to_string(), "TEST");
        assert_eq!(orders[0].order_type, OrderType::LimitOrder);
        assert_eq!(orders[0].side, Side::Buy);

        assert_eq!(trades[0].id, 123);
        assert_eq!(trades[0].cl_ord_id.to_string(), "maker123");
        assert_eq!(trades[0].price.to_f64(), 10.0);
        assert_eq!(trades[0].quantity.to_f64(), 100.0);
        assert_eq!(trades[0].order_qty.to_f64(), 100.0);
        assert_eq!(trades[0].leaves_qty.to_f64(), 0.0);

        reset_database(&pool).await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_persist_partial_fill_updates_trades_in_db() -> Result<(), sqlx::Error> {
        let _guard = TEST_MUTEX.get_or_init(|| Mutex::new(())).lock().await;

        let pool = connect_from_env().await?;
        create_tables(&pool).await?;

        reset_database(&pool).await?;

        let order_event = OrderEvent {
            price: FixedPointArithmetic::from_f64(10.5),
            quantity: FixedPointArithmetic::from_f64(100.0),
            side: Side::Buy,
            symbol: SymbolId::from_ascii("TEST"),
            order_type: OrderType::LimitOrder,
            cl_ord_id: OrderId::from_ascii("partial1"),
            orig_cl_ord_id: None,
            sender_id: EntityId::from_ascii("broker-a"),
            target_id: EntityId::from_ascii("broker-b"),
            timestamp: Instant::now(),
        };

        let mut trades = Trades::<4>::new();
        trades.add_trade(Trade {
            price: FixedPointArithmetic::from_f64(10.5),
            quantity: FixedPointArithmetic::from_f64(40.0),
            id: 456,
            cl_ord_id: OrderId::from_ascii("maker456"),
            order_qty: FixedPointArithmetic::from_f64(75.0),
            leaves_qty: FixedPointArithmetic::from_f64(35.0),
            timestamp: Instant::now(),
        }).unwrap();

        let order_result = OrderResult {
            internal_order_id: 102,
            status: OrderStatus::PartiallyFilled,
            trades,
            timestamp: Instant::now(),
        };

        persist_order_update(&pool, &order_event, &order_result).await?;

        let stored_trades = collect_all_trades(&pool).await?;
        let stored_results = collect_all_order_results(&pool).await?;

        assert_eq!(stored_trades.len(), 1);
        assert_eq!(stored_results.len(), 1);

        assert_eq!(stored_results[0].status, OrderStatus::PartiallyFilled);
        assert_eq!(stored_results[0].internal_order_id, 102);

        let trade = &stored_trades[0];
        assert_eq!(trade.id, 456);
        assert_eq!(trade.cl_ord_id.to_string(), "maker456");
        assert_eq!(trade.price.to_f64(), 10.5);
        assert_eq!(trade.quantity.to_f64(), 40.0);
        assert_eq!(trade.order_qty.to_f64(), 75.0);
        assert_eq!(trade.leaves_qty.to_f64(), 35.0);

        reset_database(&pool).await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_pending_orders_track_new_partial_and_filled_orders() -> Result<(), sqlx::Error> {
        let _guard = TEST_MUTEX.get_or_init(|| Mutex::new(())).lock().await;

        let pool = connect_from_env().await?;
        create_tables(&pool).await?;
        reset_database(&pool).await?;

        let order_event = OrderEvent {
            price: FixedPointArithmetic::from_f64(12.0),
            quantity: FixedPointArithmetic::from_f64(100.0),
            side: Side::Buy,
            symbol: SymbolId::from_ascii("TEST"),
            order_type: OrderType::LimitOrder,
            cl_ord_id: OrderId::from_ascii("pend001"),
            orig_cl_ord_id: None,
            sender_id: EntityId::from_ascii("broker-a"),
            target_id: EntityId::from_ascii("broker-b"),
            timestamp: Instant::now(),
        };

        let new_result = OrderResult {
            internal_order_id: 201,
            status: OrderStatus::New,
            trades: Trades::<4>::new(),
            timestamp: Instant::now(),
        };

        persist_order_update(&pool, &order_event, &new_result).await?;

        let pending_after_new = collect_all_pending_orders(&pool).await?;
        assert_eq!(pending_after_new.len(), 1);
        assert_eq!(pending_after_new[0].cl_ord_id.to_string(), "pend001");
        assert_eq!(pending_after_new[0].quantity.to_f64(), 100.0);

        let mut partial_trades = Trades::<4>::new();
        partial_trades.add_trade(Trade {
            price: FixedPointArithmetic::from_f64(12.0),
            quantity: FixedPointArithmetic::from_f64(40.0),
            id: 1001,
            cl_ord_id: OrderId::from_ascii("maker-aa"),
            order_qty: FixedPointArithmetic::from_f64(50.0),
            leaves_qty: FixedPointArithmetic::from_f64(10.0),
            timestamp: Instant::now(),
        }).unwrap();

        let partial_result = OrderResult {
            internal_order_id: 202,
            status: OrderStatus::PartiallyFilled,
            trades: partial_trades,
            timestamp: Instant::now(),
        };

        persist_order_update(&pool, &order_event, &partial_result).await?;

        let pending_after_partial = collect_all_pending_orders(&pool).await?;
        assert_eq!(pending_after_partial.len(), 1);
        assert_eq!(pending_after_partial[0].cl_ord_id.to_string(), "pend001");
        assert_eq!(pending_after_partial[0].quantity.to_f64(), 60.0);

        let mut fill_trades = Trades::<4>::new();
        fill_trades.add_trade(Trade {
            price: FixedPointArithmetic::from_f64(12.0),
            quantity: FixedPointArithmetic::from_f64(100.0),
            id: 1002,
            cl_ord_id: OrderId::from_ascii("maker-bb"),
            order_qty: FixedPointArithmetic::from_f64(60.0),
            leaves_qty: FixedPointArithmetic::ZERO,
            timestamp: Instant::now(),
        }).unwrap();

        let filled_result = OrderResult {
            internal_order_id: 203,
            status: OrderStatus::Filled,
            trades: fill_trades,
            timestamp: Instant::now(),
        };

        persist_order_update(&pool, &order_event, &filled_result).await?;

        let pending_after_fill = collect_all_pending_orders(&pool).await?;
        assert!(pending_after_fill.is_empty());

        reset_database(&pool).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_pending_orders_removed_on_cancel() -> Result<(), sqlx::Error> {
        let _guard = TEST_MUTEX.get_or_init(|| Mutex::new(())).lock().await;

        let pool = connect_from_env().await?;
        create_tables(&pool).await?;
        reset_database(&pool).await?;

        let live_order = OrderEvent {
            price: FixedPointArithmetic::from_f64(9.5),
            quantity: FixedPointArithmetic::from_f64(25.0),
            side: Side::Sell,
            symbol: SymbolId::from_ascii("TEST"),
            order_type: OrderType::LimitOrder,
            cl_ord_id: OrderId::from_ascii("live001"),
            orig_cl_ord_id: None,
            sender_id: EntityId::from_ascii("broker-a"),
            target_id: EntityId::from_ascii("broker-b"),
            timestamp: Instant::now(),
        };

        persist_order_update(
            &pool,
            &live_order,
            &OrderResult {
                internal_order_id: 301,
                status: OrderStatus::New,
                trades: Trades::<4>::new(),
                timestamp: Instant::now(),
            },
        ).await?;

        let cancel_event = OrderEvent {
            price: FixedPointArithmetic::from_f64(9.5),
            quantity: FixedPointArithmetic::ZERO,
            side: Side::Sell,
            symbol: SymbolId::from_ascii("TEST"),
            order_type: OrderType::CancelOrder,
            cl_ord_id: OrderId::from_ascii("cancel01"),
            orig_cl_ord_id: Some(OrderId::from_ascii("live001")),
            sender_id: EntityId::from_ascii("broker-a"),
            target_id: EntityId::from_ascii("broker-b"),
            timestamp: Instant::now(),
        };

        persist_order_update(
            &pool,
            &cancel_event,
            &OrderResult {
                internal_order_id: 302,
                status: OrderStatus::Cancelled,
                trades: Trades::<4>::new(),
                timestamp: Instant::now(),
            },
        ).await?;

        assert!(collect_all_pending_orders(&pool).await?.is_empty());

        reset_database(&pool).await?;
        Ok(())
    }
}
