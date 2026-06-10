use alloy::providers::ProviderBuilder;
use indexer::cursor::ensure_cursors;
use indexer::pipeline::index_loop;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = shared::config::AppConfig::from_env()?;
    shared::tracing_setup::init(&config.log_format);
    info!("indexer starting");

    let pool = shared::db::init_pool(&config.database_url).await?;
    let provider = ProviderBuilder::new().on_http(config.rpc_url.parse()?);

    ensure_cursors(&pool, &config).await?;

    let shutdown = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("shutdown signal received");
    });

    tokio::select! {
        _ = index_loop(pool, provider, config) => {},
        _ = shutdown => {},
    }

    info!("indexer shutting down");
    Ok(())
}
