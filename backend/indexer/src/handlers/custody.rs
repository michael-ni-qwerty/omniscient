use crate::abi::u256_to_bigdecimal;
use crate::events::HandlerCtx;
use alloy::primitives::U256;
use alloy::rpc::types::Log;
use sqlx::Postgres;
use tracing::{error, warn};

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
         ON CONFLICT (user_address, asset_type, COALESCE(position_id, 0))
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

    if log.data().data.len() < 96 {
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
         ON CONFLICT (user_address, asset_type, COALESCE(position_id, 0))
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

pub async fn handle_forced_withdrawal_requested(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    _ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let account = match log.topics().get(1) {
        Some(a) => a,
        None => return Ok(false),
    };

    if log.data().data.len() < 32 {
        warn!("ForcedWithdrawalRequested data too short");
        return Ok(false);
    }

    let ready_at_bytes: [u8; 32] = match log.data().data[0..32].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("readyAt bytes invalid");
            return Err(shared::Error::Domain("readyAt bytes invalid".into()));
        }
    };
    let ready_at = U256::from_be_bytes(ready_at_bytes);
    let _ready_at_i64 = i64::try_from(ready_at)
        .map_err(|e| shared::Error::Domain(format!("ready_at conversion error: {}", e)))?;
    let account_addr = &account.as_slice()[12..32];

    sqlx::query("INSERT INTO users (address) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(account_addr)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            error!("user insert error: {}", e);
            shared::Error::Sqlx(e)
        })?;

    sqlx::query("UPDATE users SET updated_at = now() WHERE address = $1")
        .bind(account_addr)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            error!("user update error: {}", e);
            shared::Error::Sqlx(e)
        })?;

    Ok(true)
}

pub async fn handle_forced_withdrawal_cancelled(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    _ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let account = match log.topics().get(1) {
        Some(a) => a,
        None => return Ok(false),
    };
    let account_addr = &account.as_slice()[12..32];

    sqlx::query("INSERT INTO users (address) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(account_addr)
        .execute(&mut **tx)
        .await
        .map_err(|e| {
            error!("user insert error: {}", e);
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
         ON CONFLICT (user_address, asset_type, COALESCE(position_id, 0))
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

pub async fn handle_forced_withdrawal_delay_updated(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    if log.data().data.len() < 64 {
        warn!("ForcedWithdrawalDelayUpdated data too short");
        return Ok(false);
    }

    let old_bytes: [u8; 32] = match log.data().data[0..32].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("oldDelay bytes invalid");
            return Err(shared::Error::Domain("oldDelay bytes invalid".into()));
        }
    };
    let new_bytes: [u8; 32] = match log.data().data[32..64].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("newDelay bytes invalid");
            return Err(shared::Error::Domain("newDelay bytes invalid".into()));
        }
    };
    let old_delay = U256::from_be_bytes(old_bytes);
    let new_delay = U256::from_be_bytes(new_bytes);

    sqlx::query(
        "INSERT INTO audit_log (service, event_type, payload, created_at)
         VALUES ('indexer', 'forced_withdrawal_delay_updated', $1, now())",
    )
    .bind(serde_json::json!({
        "old_delay": old_delay.to_string(),
        "new_delay": new_delay.to_string(),
        "block_number": ctx.block_number,
    }))
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("audit log insert error: {}", e);
        shared::Error::Sqlx(e)
    })?;

    Ok(true)
}

pub async fn handle_signer_approval_set(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let (maker, signer) = match (log.topics().get(1), log.topics().get(2)) {
        (Some(m), Some(s)) => (m, s),
        _ => return Ok(false),
    };
    let maker_addr = &maker.as_slice()[12..32];
    let signer_addr = &signer.as_slice()[12..32];

    if log.data().data.len() < 32 {
        warn!("SignerApprovalSet data too short");
        return Ok(false);
    }

    let approved_bytes: [u8; 32] = match log.data().data[0..32].try_into() {
        Ok(b) => b,
        Err(_) => {
            warn!("approved bytes invalid");
            return Err(shared::Error::Domain("approved bytes invalid".into()));
        }
    };
    let approved = U256::from_be_bytes(approved_bytes) != U256::ZERO;

    sqlx::query(
        "INSERT INTO audit_log (service, event_type, payload, created_at)
         VALUES ('indexer', 'signer_approval_set', $1, now())",
    )
    .bind(serde_json::json!({
        "maker": format!("0x{}", hex::encode(maker_addr)),
        "signer": format!("0x{}", hex::encode(signer_addr)),
        "approved": approved,
        "block_number": ctx.block_number,
    }))
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("audit log insert error: {}", e);
        shared::Error::Sqlx(e)
    })?;

    Ok(true)
}

pub async fn handle_fee_rates_updated(
    tx: &mut sqlx::Transaction<'_, Postgres>,
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

    sqlx::query(
        "INSERT INTO audit_log (service, event_type, payload, created_at)
         VALUES ('indexer', 'fee_rates_updated', $1, now())",
    )
    .bind(serde_json::json!({
        "taker_fee_bps": taker_fee.to_string(),
        "maker_rebate_bps": maker_rebate.to_string(),
        "block_number": ctx.block_number,
    }))
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        error!("audit log insert error: {}", e);
        shared::Error::Sqlx(e)
    })?;

    Ok(true)
}
