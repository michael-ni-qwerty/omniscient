use crate::events::HandlerCtx;
use alloy::primitives::U256;
use alloy::rpc::types::Log;
use sqlx::Postgres;
use tracing::{error, warn};

pub async fn handle_net_deltas_applied(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let batch_id_topic = match log.topics().get(1) {
        Some(b) => b,
        None => return Ok(false),
    };

    let block_i64 = i64::try_from(ctx.block_number)
        .map_err(|e| shared::Error::Domain(format!("block_number conversion error: {}", e)))?;

    sqlx::query(
        "UPDATE settlement_batches SET status = 'finalized', finalized_at = now(), block_number = $2
         WHERE batch_id = $1",
    )
    .bind(batch_id_topic.as_slice())
    .bind(block_i64)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("settlement batch update error: {}", e);
        shared::Error::Sqlx(e)
    })?;

    Ok(true)
}

pub async fn handle_nonce_invalidated(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    _ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let user = match log.topics().get(1) {
        Some(u) => u,
        None => return Ok(false),
    };
    let user_addr = &user.as_slice()[12..32];

    if log.data().data.len() < 32 {
        warn!("NonceInvalidated data too short");
        return Ok(false);
    }

    let nonce_bytes: [u8; 32] = match log.data().data[..32].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("NonceInvalidated nonce bytes invalid");
            return Err(shared::Error::Domain(
                "NonceInvalidated nonce bytes invalid".into(),
            ));
        }
    };
    let new_nonce = U256::from_be_bytes(nonce_bytes);
    let nonce_i64 = i64::try_from(new_nonce)
        .map_err(|e| shared::Error::Domain(format!("nonce conversion error: {}", e)))?;

    sqlx::query(
        "UPDATE orders SET status = 'cancelled', updated_at = now()
         WHERE user_address = $1 AND nonce < $2 AND status = 'open'",
    )
    .bind(user_addr)
    .bind(nonce_i64)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("order cancellation on nonce invalidation error: {}", e);
        shared::Error::Sqlx(e)
    })?;

    Ok(true)
}
