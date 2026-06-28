use crate::abi::parse_string;
use crate::abi::parse_u256_array;
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
        "INSERT INTO markets (market_id, question_id, resolver_id, class, state, expiry, block_number, created_at, updated_at)
         VALUES ($1, $2, 'ai', 'ai', 'open', to_timestamp($3), $4, now(), now())
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
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let market_id = match log.topics().get(1) {
        Some(m) => m,
        None => return Ok(false),
    };
    let market_id_bytes = market_id.as_slice();
    let proposer = match log.topics().get(2) {
        Some(p) => p,
        None => return Ok(false),
    };
    let proposer_addr = &proposer.as_slice()[12..32];

    let payouts = match parse_u256_array(&log.data().data) {
        Some(p) => p,
        None => return Ok(false),
    };
    let payouts_json = serde_json::to_value(&payouts).unwrap_or(serde_json::Value::Null);

    let block_i64 = i64::try_from(ctx.block_number)
        .map_err(|e| shared::Error::Domain(format!("block_number conversion error: {}", e)))?;

    sqlx::query(
        "INSERT INTO resolution_proposals (market_id, round, resolver_id, proposed_payouts, status, proposer_address, proposed_at, block_number, created_at, updated_at)
         VALUES ($1, 0, 'oracle', $2, 'proposed', $3, now(), $4, now(), now())
         ON CONFLICT (market_id, round) DO UPDATE SET
         proposed_payouts = EXCLUDED.proposed_payouts,
         status = 'proposed',
         proposer_address = EXCLUDED.proposer_address,
         proposed_at = now(),
         block_number = EXCLUDED.block_number,
         updated_at = now()",
    )
    .bind(market_id_bytes)
    .bind(payouts_json)
    .bind(proposer_addr)
    .bind(block_i64)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("resolution proposal insert error: {}", e);
        shared::Error::Sqlx(e)
    })?;

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
    let disputer = match log.topics().get(2) {
        Some(d) => d,
        None => return Ok(false),
    };
    let disputer_addr = &disputer.as_slice()[12..32];
    let reasoning = match parse_string(&log.data().data) {
        Some(r) => r,
        None => return Ok(false),
    };

    sqlx::query(
        "UPDATE resolution_proposals
         SET status = 'disputed', disputer_address = $2, dispute_reason = $3, updated_at = now()
         WHERE market_id = $1 AND round = 0",
    )
    .bind(market_id_bytes)
    .bind(disputer_addr)
    .bind(reasoning)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("resolution proposal update error: {}", e);
        shared::Error::Sqlx(e)
    })?;

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

    let payouts = match parse_u256_array(&log.data().data) {
        Some(p) => p,
        None => return Ok(false),
    };
    let payouts_json = serde_json::to_value(&payouts).unwrap_or(serde_json::Value::Null);

    sqlx::query(
        "UPDATE markets SET state = 'resolved', updated_at = now()
         WHERE market_id = $1",
    )
    .bind(market_id_bytes)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("market update error: {}", e);
        shared::Error::Sqlx(e)
    })?;

    sqlx::query(
        "UPDATE resolution_proposals SET status = 'resolved', proposed_payouts = $2, finalized_at = now(), updated_at = now()
         WHERE market_id = $1 AND round = 0",
    )
    .bind(market_id_bytes)
    .bind(payouts_json)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("resolution proposal update error: {}", e);
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

    let payouts = match parse_u256_array(&log.data().data) {
        Some(p) => p,
        None => return Ok(false),
    };
    let payouts_json = serde_json::to_value(&payouts).unwrap_or(serde_json::Value::Null);

    sqlx::query("UPDATE markets SET state = 'resolved', updated_at = now() WHERE market_id = $1")
        .bind(market_id_bytes)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            error!("market update error: {}", e);
            shared::Error::Sqlx(e)
        })?;

    sqlx::query(
        "UPDATE resolution_proposals
         SET status = 'resolved', proposed_payouts = $2, finalized_at = now(), updated_at = now()
         WHERE market_id = $1 AND round = 0",
    )
    .bind(market_id_bytes)
    .bind(payouts_json)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("resolution proposal update error: {}", e);
        shared::Error::Sqlx(e)
    })?;

    Ok(true)
}
