use sqlx::PgPool;

pub async fn ensure_cursors(
    pool: &PgPool,
    config: &shared::config::AppConfig,
) -> Result<(), shared::Error> {
    for addr in [
        config.custody_addr,
        config.settlement_exchange_addr,
        config.oracle_addr,
    ] {
        sqlx::query(
            "INSERT INTO indexer_cursors (contract_address, last_finalized_block, updated_at)
             VALUES ($1, 0, now())
             ON CONFLICT DO NOTHING",
        )
        .bind(addr.as_slice())
        .execute(pool)
        .await?;
    }
    Ok(())
}
