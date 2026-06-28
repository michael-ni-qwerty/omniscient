use crate::abi::u256_to_bigdecimal;
use crate::events::HandlerCtx;
use alloy::primitives::U256;
use alloy::rpc::types::Log;
use sqlx::Postgres;
use tracing::{error, info, warn};

pub async fn handle_deposited(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let user = match log.topics().get(1) {
        Some(u) => u,
        None => return Ok(false),
    };
    let user_addr = &user.as_slice()[12..32];

    if log.data().data.len() < 32 {
        warn!("Deposited data too short");
        return Ok(false);
    }

    let amount_bytes: [u8; 32] = match log.data().data[..32].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("Deposited amount bytes invalid");
            return Err(shared::Error::Domain(
                "Deposited amount bytes invalid".into(),
            ));
        }
    };
    let amount = U256::from_be_bytes(amount_bytes);

    sqlx::query("INSERT INTO users (address) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(user_addr)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            error!("user insert error: {}", e);
            shared::Error::Sqlx(e)
        })?;

    let amount_bd = u256_to_bigdecimal(amount);
    let block_i64 = i64::try_from(ctx.block_number)
        .map_err(|e| shared::Error::Domain(format!("block_number conversion error: {}", e)))?;

    sqlx::query(
        "INSERT INTO balances (user_address, asset_type, available_amount, finalized_block_number, updated_at)
         VALUES ($1, 'usdc', $2, $3, now())
         ON CONFLICT (user_address, asset_type, COALESCE(position_id, -1))
         DO UPDATE SET available_amount = balances.available_amount + EXCLUDED.available_amount,
                       finalized_block_number = EXCLUDED.finalized_block_number,
                       updated_at = now()",
    )
    .bind(user_addr)
    .bind(amount_bd)
    .bind(block_i64)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("balance insert error: {}", e);
        shared::Error::Sqlx(e)
    })?;

    Ok(true)
}

pub async fn handle_withdrawn(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let account = match log.topics().get(1) {
        Some(a) => a,
        None => return Ok(false),
    };
    let account_addr = &account.as_slice()[12..32];

    if log.data().data.len() < 64 {
        warn!("Withdrawn data too short");
        return Ok(false);
    }

    let amount_bytes: [u8; 32] = match log.data().data[0..32].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("Withdrawn amount bytes invalid");
            return Err(shared::Error::Domain(
                "Withdrawn amount bytes invalid".into(),
            ));
        }
    };
    let amount = U256::from_be_bytes(amount_bytes);
    let amount_bd = u256_to_bigdecimal(amount);
    let block_i64 = i64::try_from(ctx.block_number)
        .map_err(|e| shared::Error::Domain(format!("block_number conversion error: {}", e)))?;

    sqlx::query(
        "INSERT INTO balances (user_address, asset_type, available_amount, finalized_block_number, updated_at)
         VALUES ($1, 'usdc', $2, $3, now())
         ON CONFLICT (user_address, asset_type, COALESCE(position_id, -1))
         DO UPDATE SET available_amount = balances.available_amount - EXCLUDED.available_amount,
                       finalized_block_number = EXCLUDED.finalized_block_number,
                       updated_at = now()",
    )
    .bind(account_addr)
    .bind(amount_bd)
    .bind(block_i64)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("balance update error: {}", e);
        shared::Error::Sqlx(e)
    })?;

    Ok(true)
}

pub async fn handle_forced_withdrawal_executed(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let account = match log.topics().get(1) {
        Some(a) => a,
        None => return Ok(false),
    };
    let account_addr = &account.as_slice()[12..32];

    if log.data().data.len() < 32 {
        warn!("ForcedWithdrawalExecuted data too short");
        return Ok(false);
    }

    let amount_bytes: [u8; 32] = match log.data().data[0..32].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("amount bytes invalid");
            return Err(shared::Error::Domain("amount bytes invalid".into()));
        }
    };
    let amount = U256::from_be_bytes(amount_bytes);
    let amount_bd = u256_to_bigdecimal(amount);
    let block_i64 = i64::try_from(ctx.block_number)
        .map_err(|e| shared::Error::Domain(format!("block_number conversion error: {}", e)))?;

    sqlx::query(
        "INSERT INTO balances (user_address, asset_type, available_amount, finalized_block_number, updated_at)
         VALUES ($1, 'usdc', $2, $3, now())
         ON CONFLICT (user_address, asset_type, COALESCE(position_id, -1))
         DO UPDATE SET available_amount = balances.available_amount - EXCLUDED.available_amount,
                       finalized_block_number = EXCLUDED.finalized_block_number,
                       updated_at = now()",
    )
    .bind(account_addr)
    .bind(amount_bd)
    .bind(block_i64)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("balance update error: {}", e);
        shared::Error::Sqlx(e)
    })?;

    Ok(true)
}

pub async fn handle_operator_heartbeat(
    _tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    if log.data().data.len() < 32 {
        warn!("OperatorHeartbeat data too short");
        return Ok(false);
    }

    let ts_bytes: [u8; 32] = match log.data().data[0..32].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("timestamp bytes invalid");
            return Err(shared::Error::Domain("timestamp bytes invalid".into()));
        }
    };
    let timestamp = U256::from_be_bytes(ts_bytes);

    info!(
        "OperatorHeartbeat: timestamp={} block={}",
        timestamp, ctx.block_number
    );

    Ok(true)
}

pub async fn handle_operator_inactivity_threshold_updated(
    _tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    if log.data().data.len() < 32 {
        warn!("OperatorInactivityThresholdUpdated data too short");
        return Ok(false);
    }

    let threshold_bytes: [u8; 32] = match log.data().data[0..32].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("threshold bytes invalid");
            return Err(shared::Error::Domain("threshold bytes invalid".into()));
        }
    };
    let new_threshold = U256::from_be_bytes(threshold_bytes);

    info!(
        "OperatorInactivityThresholdUpdated: new_threshold={} block={}",
        new_threshold, ctx.block_number
    );

    Ok(true)
}

pub async fn handle_fee_rates_updated(
    _tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    if log.data().data.len() < 64 {
        warn!("FeeRatesUpdated data too short");
        return Ok(false);
    }

    let taker_bytes: [u8; 32] = match log.data().data[0..32].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("takerFeeBps bytes invalid");
            return Err(shared::Error::Domain("takerFeeBps bytes invalid".into()));
        }
    };
    let maker_bytes: [u8; 32] = match log.data().data[32..64].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("makerRebateBps bytes invalid");
            return Err(shared::Error::Domain("makerRebateBps bytes invalid".into()));
        }
    };
    let taker_fee = U256::from_be_bytes(taker_bytes);
    let maker_rebate = U256::from_be_bytes(maker_bytes);

    info!(
        "FeeRatesUpdated: taker_fee_bps={} maker_rebate_bps={} block={}",
        taker_fee, maker_rebate, ctx.block_number
    );

    Ok(true)
}
