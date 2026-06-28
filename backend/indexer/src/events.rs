use crate::abi::topic_hash;
use crate::handlers::{custody, oracle, settlement};
use alloy::primitives::B256;
use alloy::rpc::types::Log;
use sqlx::Postgres;
use std::sync::LazyLock;
use tracing::{info, warn};

pub struct HandlerCtx<'a> {
    pub block_number: u64,
    pub config: &'a shared::config::AppConfig,
    pub addr_bytes: &'a [u8],
}

macro_rules! define_topic {
    ($name:ident, $sig:expr) => {
        static $name: LazyLock<B256> = LazyLock::new(|| topic_hash($sig.as_bytes()));
    };
}

define_topic!(DEPOSITED, "Deposited(address,uint256,uint256)");
define_topic!(WITHDRAWN, "Withdrawn(address,uint256,uint256)");
define_topic!(
    FORCED_WITHDRAWAL_EXECUTED,
    "ForcedWithdrawalExecuted(address,address,uint256)"
);
define_topic!(OPERATOR_HEARTBEAT, "OperatorHeartbeat(uint256)");
define_topic!(
    OPERATOR_INACTIVITY_THRESHOLD_UPDATED,
    "OperatorInactivityThresholdUpdated(uint256)"
);
define_topic!(FEE_RATES_UPDATED, "FeeRatesUpdated(uint256,uint256)");
define_topic!(NONCE_INVALIDATED, "NonceInvalidated(address,uint256)");
define_topic!(OUTCOME_RESOLVED, "OutcomeResolved(bytes32,uint256[])");
define_topic!(MARKET_CREATED, "MarketCreated(bytes32,bytes32,uint256)");
define_topic!(
    OUTCOME_PROPOSED,
    "OutcomeProposed(bytes32,address,uint256[])"
);
define_topic!(OUTCOME_DISPUTED, "OutcomeDisputed(bytes32,address,string)");
define_topic!(DISPUTE_RESOLVED, "DisputeResolved(bytes32,uint256[])");

pub async fn dispatch(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    log: &Log,
    ctx: &HandlerCtx<'_>,
) -> Result<bool, shared::Error> {
    let topic0 = match log.topics().first() {
        Some(t) => t,
        None => {
            warn!("log missing topics");
            return Ok(false);
        }
    };

    info!("log topic0={} block={}", topic0, ctx.block_number);

    if *topic0 == *DEPOSITED {
        custody::handle_deposited(tx, log, ctx).await
    } else if *topic0 == *WITHDRAWN {
        custody::handle_withdrawn(tx, log, ctx).await
    } else if *topic0 == *FORCED_WITHDRAWAL_EXECUTED {
        custody::handle_forced_withdrawal_executed(tx, log, ctx).await
    } else if *topic0 == *OPERATOR_HEARTBEAT {
        custody::handle_operator_heartbeat(tx, log, ctx).await
    } else if *topic0 == *OPERATOR_INACTIVITY_THRESHOLD_UPDATED {
        custody::handle_operator_inactivity_threshold_updated(tx, log, ctx).await
    } else if *topic0 == *FEE_RATES_UPDATED {
        custody::handle_fee_rates_updated(tx, log, ctx).await
    } else if *topic0 == *NONCE_INVALIDATED {
        settlement::handle_nonce_invalidated(tx, log, ctx).await
    } else if *topic0 == *MARKET_CREATED {
        oracle::handle_market_created(tx, log, ctx).await
    } else if *topic0 == *OUTCOME_PROPOSED {
        oracle::handle_outcome_proposed(tx, log, ctx).await
    } else if *topic0 == *OUTCOME_DISPUTED {
        oracle::handle_outcome_disputed(tx, log, ctx).await
    } else if *topic0 == *OUTCOME_RESOLVED {
        oracle::handle_outcome_resolved(tx, log, ctx).await
    } else if *topic0 == *DISPUTE_RESOLVED {
        oracle::handle_dispute_resolved(tx, log, ctx).await
    } else {
        warn!("unknown event topic0: {}", topic0);
        Ok(false)
    }
}
