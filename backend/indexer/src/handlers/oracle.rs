use crate::events::HandlerCtx;
use alloy::primitives::U256;
use alloy::rpc::types::Log;
use sqlx::Postgres;
use tracing::{error, warn};

pub async fn handle_market_created(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let market_id = match log.topics().get(1) {
        Some(m) => m,
        None => return Ok(false),
    };
    let market_id_bytes = market_id.as_slice();

    if log.data().data.len() < 64 {
        warn!("MarketCreated data too short");
        return Ok(false);
    }

    let question_id_bytes: [u8; 32] = match log.data().data[0..32].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("questionId bytes invalid");
            return Err(shared::Error::Domain("questionId bytes invalid".into()));
        }
    };
    let expiry_bytes: [u8; 32] = match log.data().data[32..64].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("expiry bytes invalid");
            return Err(shared::Error::Domain("expiry bytes invalid".into()));
        }
    };
    let question_id = U256::from_be_bytes(question_id_bytes);
    let expiry = U256::from_be_bytes(expiry_bytes);
    let expiry_i64 = i64::try_from(expiry)
        .map_err(|e| shared::Error::Domain(format!("expiry conversion error: {}", e)))?;

    let block_i64 = i64::try_from(ctx.block_number)
        .map_err(|e| shared::Error::Domain(format!("block_number conversion error: {}", e)))?;

    sqlx::query(
        "INSERT INTO markets (market_id, question_id, state, expiry, block_number, created_at, updated_at)
         VALUES ($1, $2, 'open', to_timestamp($3), $4, now(), now())
         ON CONFLICT (market_id) DO NOTHING",
    )
    .bind(market_id_bytes)
    .bind(question_id.to_string())
    .bind(expiry_i64)
    .bind(block_i64)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("market insert error: {}", e);
        shared::Error::Sqlx(e)
    })?;

    Ok(true)
}

pub async fn handle_outcome_proposed(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    _ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let market_id = match log.topics().get(1) {
        Some(m) => m,
        None => return Ok(false),
    };
    let market_id_bytes = market_id.as_slice();

    sqlx::query("UPDATE markets SET state = 'proposed', updated_at = now() WHERE market_id = $1")
        .bind(market_id_bytes)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            error!("market state update error: {}", e);
            shared::Error::Sqlx(e)
        })?;

    Ok(true)
}

pub async fn handle_outcome_disputed(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    _ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let market_id = match log.topics().get(1) {
        Some(m) => m,
        None => return Ok(false),
    };
    let market_id_bytes = market_id.as_slice();

    sqlx::query("UPDATE markets SET state = 'disputed', updated_at = now() WHERE market_id = $1")
        .bind(market_id_bytes)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            error!("market state update error: {}", e);
            shared::Error::Sqlx(e)
        })?;

    Ok(true)
}

pub async fn handle_outcome_resolved(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    _ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let market_id = match log.topics().get(1) {
        Some(m) => m,
        None => return Ok(false),
    };
    let market_id_bytes = market_id.as_slice();

    sqlx::query("UPDATE markets SET state = 'resolved', updated_at = now() WHERE market_id = $1")
        .bind(market_id_bytes)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            error!("market update error: {}", e);
            shared::Error::Sqlx(e)
        })?;

    Ok(true)
}

pub async fn handle_dispute_resolved(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    _ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let market_id = match log.topics().get(1) {
        Some(m) => m,
        None => return Ok(false),
    };
    let market_id_bytes = market_id.as_slice();

    sqlx::query("UPDATE markets SET state = 'resolved', updated_at = now() WHERE market_id = $1")
        .bind(market_id_bytes)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            error!("market update error: {}", e);
            shared::Error::Sqlx(e)
        })?;

    Ok(true)
}
