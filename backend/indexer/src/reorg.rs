use sqlx::PgPool;
use tracing::info;

pub async fn rollback_unfinalized(pool: &PgPool, from_block: u64) -> Result<(), shared::Error> {
    let from_i64 = i64::try_from(from_block)
        .map_err(|e| shared::Error::Domain(format!("from_block conversion: {}", e)))?;
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM reorg_checkpoints WHERE block_number >= $1 AND is_finalized = FALSE")
        .bind(from_i64)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM indexed_logs WHERE block_number >= $1")
        .bind(from_i64)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM balances WHERE finalized_block_number >= $1")
        .bind(from_i64)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM order_cancellations WHERE block_number >= $1")
        .bind(from_i64)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM markets WHERE block_number >= $1")
        .bind(from_i64)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM settlement_batches WHERE block_number >= $1")
        .bind(from_i64)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM resolution_proposals WHERE block_number >= $1")
        .bind(from_i64)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    info!("rolled back unfinalized state from block {}", from_block);
    Ok(())
}
