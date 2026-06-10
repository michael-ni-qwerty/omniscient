use crate::events::{dispatch, HandlerCtx};
use crate::finalization::finalize_blocks;
use alloy::primitives::B256;
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use sqlx::PgPool;
use std::time::Duration;
use tokio::time::interval;
use tracing::{error, info, warn};

pub async fn index_loop(
    pool: PgPool,
    provider: alloy::providers::RootProvider<alloy::transports::http::Http<reqwest::Client>>,
    config: shared::config::AppConfig,
) {
    let mut ticker = interval(Duration::from_secs(2));
    loop {
        ticker.tick().await;

        let current_block = match provider.get_block_number().await {
            Ok(n) => n,
            Err(e) => {
                warn!("get_block_number error: {}", e);
                continue;
            }
        };

        let cursors = match sqlx::query_as::<_, (Vec<u8>, i64)>(
            "SELECT contract_address, last_finalized_block FROM indexer_cursors",
        )
        .fetch_all(&pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                error!("cursor query error: {}", e);
                continue;
            }
        };

        for (addr_bytes, last_block) in cursors {
            let start = (last_block + 1) as u64;
            if start > current_block {
                continue;
            }
            let end = (start + 50).min(current_block);

            let filter = Filter::new()
                .address(alloy::primitives::Address::from_slice(&addr_bytes))
                .from_block(start)
                .to_block(end);

            let logs = match provider.get_logs(&filter).await {
                Ok(l) => l,
                Err(e) => {
                    warn!("get_logs error: {}", e);
                    continue;
                }
            };

            for log in logs {
                let block_number = match log.block_number {
                    Some(n) => n,
                    None => {
                        warn!("log missing block number, skipping");
                        continue;
                    }
                };
                let block_hash = log.block_hash.unwrap_or(B256::ZERO);
                let tx_hash = log.transaction_hash.unwrap_or(B256::ZERO);
                let log_index = log.log_index.unwrap_or(0);

                let log_index_i64 = match i64::try_from(log_index) {
                    Ok(i) => i,
                    Err(e) => {
                        error!("log_index conversion error: {}", e);
                        continue;
                    }
                };
                let already_processed = match sqlx::query_as::<_, (Vec<u8>, i64)>(
                    "SELECT tx_hash, log_index FROM indexed_logs WHERE tx_hash = $1 AND log_index = $2",
                )
                .bind(tx_hash.as_slice())
                .bind(log_index_i64)
                .fetch_optional(&pool)
                .await
                {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(e) => {
                        error!("idempotency check error: {}", e);
                        continue;
                    }
                };
                if already_processed {
                    continue;
                }

                let mut tx = match pool.begin().await {
                    Ok(t) => t,
                    Err(e) => {
                        error!("tx begin error: {}", e);
                        continue;
                    }
                };

                if let Err(e) = sqlx::query(
                    "INSERT INTO reorg_checkpoints (block_number, block_hash, is_finalized, inserted_at)
                     VALUES ($1, $2, FALSE, now())
                     ON CONFLICT (block_number) DO UPDATE SET
                     block_hash = EXCLUDED.block_hash,
                     is_finalized = FALSE
                     WHERE reorg_checkpoints.is_finalized = FALSE",
                )
                .bind(match i64::try_from(block_number) {
                    Ok(i) => i,
                    Err(e) => {
                        error!("block_number conversion error: {}", e);
                        let _ = tx.rollback().await;
                        continue;
                    }
                })
                .bind(block_hash.as_slice())
                .execute(&mut *tx)
                .await
                {
                    error!("reorg checkpoint insert error: {}", e);
                    let _ = tx.rollback().await;
                    continue;
                }

                let ctx = HandlerCtx {
                    block_number,
                    config: &config,
                    addr_bytes: &addr_bytes,
                };

                let handled = match dispatch(&mut tx, &log, &ctx).await {
                    Ok(flag) => flag,
                    Err(e) => {
                        error!("handler error: {}", e);
                        let _ = tx.rollback().await;
                        continue;
                    }
                };

                if handled {
                    if let Err(e) = sqlx::query(
                        "INSERT INTO indexed_logs (block_number, tx_hash, log_index, contract_address, topic0)
                         VALUES ($1, $2, $3, $4, $5)
                         ON CONFLICT (tx_hash, log_index) DO NOTHING",
                    )
                    .bind(match i64::try_from(block_number) {
                        Ok(i) => i,
                        Err(e) => {
                            error!("block_number conversion error: {}", e);
                            let _ = tx.rollback().await;
                            continue;
                        }
                    })
                    .bind(tx_hash.as_slice())
                    .bind(match i64::try_from(log_index) {
                        Ok(i) => i,
                        Err(e) => {
                            error!("log_index conversion error: {}", e);
                            let _ = tx.rollback().await;
                            continue;
                        }
                    })
                    .bind(addr_bytes.as_slice())
                    .bind(log.topics().first().map(|t| t.as_slice()).unwrap_or(&[]))
                    .execute(&mut *tx)
                    .await
                    {
                        error!("indexed_logs insert error: {}", e);
                        let _ = tx.rollback().await;
                        continue;
                    }
                }

                if let Err(e) = tx.commit().await {
                    error!("tx commit error: {}", e);
                } else {
                    info!("processed log at block {}", block_number);
                }
            }

            if let Err(e) = finalize_blocks(
                &pool,
                &provider,
                &config,
                &addr_bytes,
                start,
                end,
                current_block,
            )
            .await
            {
                error!("finalization error: {}", e);
            }
        }
    }
}
