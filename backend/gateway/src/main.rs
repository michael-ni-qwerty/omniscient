mod api;
mod domain;
mod engine;
mod infra;
mod service;

use service::AppState;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = shared::config::AppConfig::from_env()?;
    shared::tracing_setup::init(&config.log_format);

    info!("gateway starting");

    let pool = shared::db::init_pool(&config.database_url).await?;
    let producer = shared::kafka::producer(&config.kafka_brokers)?;

    let initial_books = service::rebuild_books(&pool).await?;
    info!("loaded {} order books from DB", initial_books.len());

    let (match_tx, match_rx) = crossbeam::channel::bounded::<engine::MatchCommand>(10_000);
    let (result_tx, result_rx) = tokio::sync::mpsc::unbounded_channel();

    let matcher_handle = std::thread::Builder::new()
        .name("matcher".to_string())
        .spawn(move || {
            engine::run_matcher(match_rx, result_tx, initial_books);
        })
        .expect("spawn matcher thread");

    service::spawn_result_processor(result_rx, pool.clone(), producer.clone());

    let (ws_tx, _ws_rx) = tokio::sync::broadcast::channel::<serde_json::Value>(1024);

    let ws_consumer = shared::kafka::consumer(
        &config.kafka_brokers,
        "gateway-ws-group",
        &["orders.matched"],
    )?;
    tokio::spawn(api::ws_broadcast_loop(ws_consumer, ws_tx.clone()));

    let state = AppState {
        pool: pool.clone(),
        producer: producer.clone(),
        config: config.clone(),
        match_tx,
        ws_tx,
    };

    let app = api::router(state.clone());
    let listener = tokio::net::TcpListener::bind(&config.gateway_bind).await?;
    info!("gateway listening on {}", config.gateway_bind);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    let _ = state.match_tx.send(engine::MatchCommand::Shutdown);
    let _ = matcher_handle.join();
    info!("gateway shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("received SIGTERM, starting graceful shutdown");
}
