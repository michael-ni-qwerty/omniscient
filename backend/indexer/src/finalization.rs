use alloy::providers::Provider;
use alloy::rpc::types::{BlockId, BlockNumberOrTag, BlockTransactionsKind};
use sqlx::PgPool;
use tracing::{error, warn};

pub async fn finalize_blocks(
    pool: &PgPool,
    provider: &alloy::providers::RootProvider<alloy::transports::http::Http<reqwest::Client>>,
    config: &shared::config::AppConfig,
    addr_bytes: &[u8],
    start: u64,
    end: u64,
    current_block: u64,
) -> Result<(), shared::Error> {
    let finalized = (start..=end)
        .filter(|b| current_block.saturating_sub(*b) >= shared::constants::FINALITY_BLOCKS)
        .collect::<Vec<u64>>();

    for block_num in finalized {
        let block_hash = match provider
            .get_block(
                BlockId::Number(BlockNumberOrTag::Number(block_num)),
                BlockTransactionsKind::Hashes,
            )
            .await
        {
            Ok(Some(block)) => block.header.hash,
            _ => {
                warn!("could not fetch block {}", block_num);
                continue;
            }
        };

        let block_i64 = match i64::try_from(block_num) {
            Ok(i) => i,
            Err(e) => {
                error!("block_num conversion error: {}", e);
                continue;
            }
        };

        let stored = match sqlx::query_as::<_, (Vec<u8>,)>(
            "SELECT block_hash FROM reorg_checkpoints WHERE block_number = $1",
        )
        .bind(block_i64)
        .fetch_optional(pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                error!("reorg check error: {}", e);
                continue;
            }
        };

        if let Some((stored_hash,)) = stored {
            if stored_hash != block_hash.as_slice() {
                warn!("reorg detected at block {}", block_num);
                if let Err(e) = crate::reorg::rollback_unfinalized(pool, block_num).await {
                    error!("rollback error: {}", e);
                }
                continue;
            }
        }

        let mut tx = match pool.begin().await {
            Ok(t) => t,
            Err(e) => {
                error!("tx begin error: {}", e);
                continue;
            }
        };

        if let Err(e) =
            sqlx::query("UPDATE reorg_checkpoints SET is_finalized = TRUE WHERE block_number = $1")
                .bind(block_i64)
                .execute(&mut *tx)
                .await
        {
            error!("reorg checkpoint finalize error: {}", e);
            let _ = tx.rollback().await;
            continue;
        }

        if let Err(e) = sqlx::query(
            "UPDATE indexer_cursors
             SET last_finalized_block = $1, last_finalized_block_hash = $2, updated_at = now()
             WHERE contract_address = $3",
        )
        .bind(block_i64)
        .bind(block_hash.as_slice())
        .bind(addr_bytes)
        .execute(&mut *tx)
        .await
        {
            error!("cursor update error: {}", e);
            let _ = tx.rollback().await;
            continue;
        }

        if let Err(e) = tx.commit().await {
            error!("finalization tx commit error: {}", e);
            continue;
        }

        if addr_bytes == config.custody_addr.as_slice() {
            let deposits = match sqlx::query_as::<_, (Vec<u8>, bigdecimal::BigDecimal, i64)>(
                "SELECT user_address, available_amount, finalized_block_number
                 FROM balances WHERE finalized_block_number = $1",
            )
            .bind(block_i64)
            .fetch_all(pool)
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    error!("deposit query error: {}", e);
                    continue;
                }
            };

            for (user_addr, deposit, finalized_bn) in deposits {
                let payload = serde_json::json!({
                    "user": format!("0x{}", hex::encode(&user_addr)),
                    "deposit": deposit,
                    "block_number": finalized_bn,
                });
                let client = reqwest::Client::new();
                let url = format!("http://{}/internal/reconcile", config.gateway_bind);
                let mut last_err = None;
                for attempt in 0..3u32 {
                    match client
                        .post(&url)
                        .header("x-internal-secret", &config.gateway_internal_secret)
                        .json(&payload)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            if resp.status().is_success() {
                                last_err = None;
                                break;
                            }
                            last_err = Some(format!("status {}", resp.status()));
                        }
                        Err(e) => {
                            last_err = Some(e.to_string());
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200 * 2_u64.pow(attempt)))
                        .await;
                }
                if let Some(e) = last_err {
                    warn!("reconcile post failed after retries: {}", e);
                }
            }
        }
    }

    Ok(())
}
