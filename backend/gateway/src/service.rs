use crate::domain::{MatchResult, OrderBook, RestingOrder};
use crate::engine::MatchCommand;
use crate::infra;
use rdkafka::producer::FutureProducer;
use shared::config::AppConfig;
use shared::domain::{Address, MarketId, OrderId, SignedOrder};
use sqlx::PgPool;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{error, info};

/// Shared state for all HTTP/WS handlers.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub producer: FutureProducer,
    pub config: AppConfig,
    pub match_tx: crossbeam::channel::Sender<MatchCommand>,
    pub ws_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
}

/// Validate a signed order: verify EIP-712 signature, check deadline, check nonce.
/// Returns the recovered maker address on success.
pub fn validate_signed_order(signed: &SignedOrder, config: &AppConfig) -> Result<Address, String> {
    let now = chrono::Utc::now().timestamp() as u64;
    if signed.order.deadline <= now {
        return Err("order expired".to_string());
    }

    if signed.order.price == 0 || signed.order.price > shared::constants::PRICE_SCALE {
        return Err("price out of range (0, 1e6]".to_string());
    }

    if signed.order.amount == 0 {
        return Err("amount must be > 0".to_string());
    }

    let recovered = shared::domain::verify_order_signature(
        &signed.order,
        &signed.signature,
        config.chain_id,
        config.settlement_exchange_addr,
    )
    .map_err(|e| format!("signature verification: {e}"))?;

    if recovered.as_slice() != signed.order.maker.0 {
        return Err("signature does not match maker".to_string());
    }

    Ok(signed.order.maker)
}

/// Full order acceptance flow: validate → ensure user → reserve collateral →
/// insert order → insert hold → enqueue to matcher.
pub async fn accept_order(
    state: &AppState,
    signed: &SignedOrder,
    market_id: &MarketId,
) -> Result<OrderId, String> {
    let maker = validate_signed_order(signed, &state.config)?;

    infra::ensure_user(&state.pool, &maker)
        .await
        .map_err(|e| format!("ensure_user: {e}"))?;

    let order_id = OrderId::new();
    let collateral = signed.order.required_collateral();
    let asset_type = signed.order.asset_type();
    let position_id = &signed.order.position_id;

    let position_ref = if asset_type == "ctf" {
        Some(position_id)
    } else {
        None
    };

    infra::reserve_collateral(
        &state.pool,
        &maker,
        asset_type,
        position_ref,
        collateral as u64,
    )
    .await
    .map_err(|e| format!("collateral: {e}"))?;

    let deadline_i64 = i64::try_from(signed.order.deadline).unwrap_or(0);
    let salt_bytes = signed.order.salt.0;

    if let Err(e) = infra::insert_order(
        &state.pool,
        order_id,
        &maker,
        market_id,
        signed.order.side,
        signed.order.price,
        signed.order.amount,
        signed.order.nonce,
        &salt_bytes,
        position_id,
        &signed.signature,
        deadline_i64,
    )
    .await
    {
        let _ = infra::release_collateral(
            &state.pool,
            &maker,
            asset_type,
            position_ref,
            collateral as u64,
        )
        .await;
        return Err(format!("insert_order: {e}"));
    }

    if let Err(e) = infra::insert_hold(
        &state.pool,
        order_id,
        &maker,
        asset_type,
        position_ref,
        collateral as u64,
    )
    .await
    {
        let _ = infra::release_collateral(
            &state.pool,
            &maker,
            asset_type,
            position_ref,
            collateral as u64,
        )
        .await;
        return Err(format!("insert_hold: {e}"));
    }

    let cmd = MatchCommand::PlaceOrder {
        market_id: *market_id,
        position_id: *position_id,
        order_id,
        maker,
        price: signed.order.price,
        amount: signed.order.amount,
        side: signed.order.side,
        nonce: signed.order.nonce,
        deadline: signed.order.deadline,
    };

    state
        .match_tx
        .try_send(cmd)
        .map_err(|e| format!("matcher queue full: {e}"))?;

    Ok(order_id)
}

/// Cancel an order by order_id.
pub async fn cancel_order(
    state: &AppState,
    order_id: OrderId,
    position_id: &alloy::primitives::U256,
) -> Result<(), String> {
    state
        .match_tx
        .try_send(MatchCommand::CancelOrder {
            position_id: *position_id,
            order_id,
        })
        .map_err(|e| format!("matcher queue full: {e}"))?;

    infra::cancel_order_db(&state.pool, order_id)
        .await
        .map_err(|e| format!("cancel_order_db: {e}"))?;

    Ok(())
}

/// Process match results from the engine: persist fills, publish to Kafka,
/// update order statuses, release collateral on fills.
pub async fn process_match_result(pool: &PgPool, producer: &FutureProducer, result: MatchResult) {
    for fill in &result.fills {
        if let Err(e) = infra::publish_match(
            producer,
            &result.market_id,
            &fill.maker,
            &fill.taker,
            fill.price,
            fill.amount,
            fill.taker_side,
        )
        .await
        {
            error!("failed to publish match: {}", e);
        }

        let maker_filled = fill.amount;
        if let Err(e) =
            infra::update_order_filled(pool, fill.maker_order_id, maker_filled, true).await
        {
            error!("failed to update maker order: {}", e);
        }

        let taker_filled = fill.amount;
        if let Err(e) =
            infra::update_order_filled(pool, fill.taker_order_id, taker_filled, false).await
        {
            error!("failed to update taker order: {}", e);
        }
    }

    if let Some(resting) = &result.resting {
        let remaining = resting.remaining();
        if remaining > 0 {
            info!(
                "order {} resting with {} remaining",
                resting.order_id, remaining
            );
        }
    }
}

/// Rebuild order books from DB on startup.
pub async fn rebuild_books(
    pool: &PgPool,
) -> Result<HashMap<alloy::primitives::U256, OrderBook>, shared::Error> {
    let open_orders = infra::load_open_orders(pool).await?;
    let mut books: HashMap<alloy::primitives::U256, OrderBook> = HashMap::new();

    info!(
        "rebuilding order books from {} open orders",
        open_orders.len()
    );

    for row in open_orders {
        let book = books.entry(row.position_id).or_default();
        let mut resting = RestingOrder {
            order_id: row.order_id,
            maker: row.user_address,
            price: row.price,
            amount: row.amount,
            filled: row.filled_amount,
            side: row.side,
            seq: 0,
            nonce: row.nonce,
            deadline: row.deadline as u64,
        };
        book.insert(&mut resting);
    }

    for book in books.values_mut() {
        book.prune();
    }

    Ok(books)
}

/// Spawn the result processor task that consumes MatchResult from the engine
/// and persists/publishes them.
pub fn spawn_result_processor(
    mut result_rx: mpsc::UnboundedReceiver<MatchResult>,
    pool: PgPool,
    producer: FutureProducer,
) {
    tokio::spawn(async move {
        while let Some(result) = result_rx.recv().await {
            process_match_result(&pool, &producer, result).await;
        }
        info!("result processor task exiting");
    });
}
