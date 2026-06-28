use rdkafka::producer::FutureProducer;
use rdkafka::producer::FutureRecord;
use serde::{Deserialize, Serialize};
use shared::domain::{Address, MarketId, OrderId, OrderSide};
use sqlx::PgPool;
use tracing::warn;

/// MatchEvent published to Redpanda `orders.matched`.
/// Schema matches what the Settlement Service consumer expects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchEvent {
    pub market_id: String,
    pub maker: String,
    pub taker: String,
    pub price: u64,
    pub amount: u64,
    pub side: String,
}

/// Open order row from DB (for book rebuild on startup).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct OpenOrderRow {
    pub order_id: OrderId,
    pub user_address: Address,
    pub market_id: MarketId,
    pub side: OrderSide,
    pub price: u64,
    pub amount: u64,
    pub filled_amount: u64,
    pub nonce: u64,
    pub deadline: i64,
    pub position_id: alloy::primitives::U256,
}

/// Insert an order into the DB.
#[allow(clippy::too_many_arguments)]
pub async fn insert_order(
    pool: &PgPool,
    order_id: OrderId,
    maker: &Address,
    market_id: &MarketId,
    side: OrderSide,
    price: u64,
    amount: u64,
    nonce: u64,
    salt: &[u8; 32],
    position_id: &alloy::primitives::U256,
    signature: &[u8],
    deadline: i64,
) -> Result<(), shared::Error> {
    let side_str = match side {
        OrderSide::Buy => "buy",
        OrderSide::Sell => "sell",
    };
    sqlx::query(
        "INSERT INTO orders
         (order_id, user_address, market_id, side, price, amount, filled_amount, status,
          nonce, salt, position_id, signature, deadline, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, 0, 'open', $7, $8, $9, $10, to_timestamp($11), now(), now())",
    )
    .bind(order_id.0)
    .bind(&maker.0[..])
    .bind(&market_id.0[..])
    .bind(side_str)
    .bind(i64::try_from(price).unwrap_or(0))
    .bind(i64::try_from(amount).unwrap_or(0))
    .bind(i64::try_from(nonce).unwrap_or(0))
    .bind(salt.as_slice())
    .bind(position_id.to_string())
    .bind(signature)
    .bind(deadline)
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert a hold for an order.
pub async fn insert_hold(
    pool: &PgPool,
    order_id: OrderId,
    maker: &Address,
    asset_type: &str,
    position_id: Option<&alloy::primitives::U256>,
    amount: u64,
) -> Result<(), shared::Error> {
    let position_str = position_id.map(|p| p.to_string());
    sqlx::query(
        "INSERT INTO holds (user_address, order_id, asset_type, position_id, amount, status, created_at)
         VALUES ($1, $2, $3, $4, $5, 'held', now())",
    )
    .bind(&maker.0[..])
    .bind(order_id.0)
    .bind(asset_type)
    .bind(position_str)
    .bind(i64::try_from(amount).unwrap_or(0))
    .execute(pool)
    .await?;
    Ok(())
}

/// Atomically reserve collateral: increment hold_amount, decrement available_amount
/// in a single UPDATE. Returns Ok if sufficient balance, Err otherwise.
pub async fn reserve_collateral(
    pool: &PgPool,
    maker: &Address,
    asset_type: &str,
    position_id: Option<&alloy::primitives::U256>,
    amount: u64,
) -> Result<(), shared::Error> {
    let position_str = position_id.map(|p| p.to_string());
    let amount_i64 = i64::try_from(amount).unwrap_or(0);

    let result = sqlx::query(
        "UPDATE balances
         SET hold_amount = hold_amount + $3,
             available_amount = available_amount - $3,
             updated_at = now()
         WHERE user_address = $1
           AND asset_type = $2
           AND COALESCE(position_id::text, '') = COALESCE($4, '')
           AND available_amount >= $3",
    )
    .bind(&maker.0[..])
    .bind(asset_type)
    .bind(amount_i64)
    .bind(position_str)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        return Err(shared::Error::Domain(format!(
            "insufficient balance for {} {}",
            maker, asset_type
        )));
    }
    Ok(())
}

/// Release collateral on cancel or partial fill.
pub async fn release_collateral(
    pool: &PgPool,
    maker: &Address,
    asset_type: &str,
    position_id: Option<&alloy::primitives::U256>,
    amount: u64,
) -> Result<(), shared::Error> {
    let position_str = position_id.map(|p| p.to_string());
    let amount_i64 = i64::try_from(amount).unwrap_or(0);

    sqlx::query(
        "UPDATE balances
         SET hold_amount = GREATEST(hold_amount - $3, 0),
             available_amount = available_amount + $3,
             updated_at = now()
         WHERE user_address = $1
           AND asset_type = $2
           AND COALESCE(position_id::text, '') = COALESCE($4, '')",
    )
    .bind(&maker.0[..])
    .bind(asset_type)
    .bind(amount_i64)
    .bind(position_str)
    .execute(pool)
    .await?;
    Ok(())
}

/// Update order filled amount and status after a match.
pub async fn update_order_filled(
    pool: &PgPool,
    order_id: OrderId,
    filled_delta: u64,
    fully_filled: bool,
) -> Result<(), shared::Error> {
    let status = if fully_filled { "filled" } else { "open" };
    sqlx::query(
        "UPDATE orders
         SET filled_amount = filled_amount + $2,
             status = $3,
             updated_at = now()
         WHERE order_id = $1",
    )
    .bind(order_id.0)
    .bind(i64::try_from(filled_delta).unwrap_or(0))
    .bind(status)
    .execute(pool)
    .await?;
    Ok(())
}

/// Cancel an order in the DB.
pub async fn cancel_order_db(pool: &PgPool, order_id: OrderId) -> Result<(), shared::Error> {
    sqlx::query(
        "UPDATE orders SET status = 'cancelled', updated_at = now()
         WHERE order_id = $1 AND status = 'open'",
    )
    .bind(order_id.0)
    .execute(pool)
    .await?;
    Ok(())
}

/// Load all open orders for book rebuild on startup.
pub async fn load_open_orders(pool: &PgPool) -> Result<Vec<OpenOrderRow>, shared::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            uuid::Uuid,
            Vec<u8>,
            Vec<u8>,
            String,
            i64,
            i64,
            i64,
            i64,
            i64,
            String,
        ),
    >(
        "SELECT order_id, user_address, market_id, side, price, amount, filled_amount,
                nonce, EXTRACT(EPOCH FROM deadline)::int8, position_id::text
         FROM orders
         WHERE status = 'open'
         ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;

    let mut orders = Vec::new();
    for (
        order_id,
        user_bytes,
        market_bytes,
        side_str,
        price,
        amount,
        filled,
        nonce,
        deadline,
        pos_str,
    ) in rows
    {
        let order_id = OrderId(order_id);
        let user_address = Address::try_from(&user_bytes[..])?;
        let market_id = MarketId::try_from(&market_bytes[..])?;
        let side = match side_str.as_str() {
            "buy" => OrderSide::Buy,
            "sell" => OrderSide::Sell,
            _ => continue,
        };
        let position_id = pos_str
            .parse::<alloy::primitives::U256>()
            .unwrap_or_default();

        orders.push(OpenOrderRow {
            order_id,
            user_address,
            market_id,
            side,
            price: price as u64,
            amount: amount as u64,
            filled_amount: filled as u64,
            nonce: nonce as u64,
            deadline,
            position_id,
        });
    }
    Ok(orders)
}

/// Publish a match event to Redpanda.
pub async fn publish_match(
    producer: &FutureProducer,
    market_id: &MarketId,
    maker: &Address,
    taker: &Address,
    price: u64,
    amount: u64,
    taker_side: OrderSide,
) -> Result<(), shared::Error> {
    let event = MatchEvent {
        market_id: market_id.to_string(),
        maker: maker.to_string(),
        taker: taker.to_string(),
        price,
        amount,
        side: match taker_side {
            OrderSide::Buy => "buy".to_string(),
            OrderSide::Sell => "sell".to_string(),
        },
    };

    let payload = serde_json::to_string(&event)?;
    let key = market_id.to_string();

    let record = FutureRecord::to("orders.matched")
        .payload(payload.as_bytes())
        .key(key.as_bytes());

    producer
        .send(record, std::time::Duration::from_secs(5))
        .await
        .map_err(|(e, _)| {
            warn!("kafka produce error: {}", e);
            shared::Error::Kafka(e)
        })?;

    Ok(())
}

/// Ensure the user exists in the users table.
pub async fn ensure_user(pool: &PgPool, maker: &Address) -> Result<(), shared::Error> {
    sqlx::query("INSERT INTO users (address) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(&maker.0[..])
        .execute(pool)
        .await?;
    Ok(())
}
