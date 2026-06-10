use axum::{
    extract::{State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures::StreamExt;
use rdkafka::consumer::Consumer;
use rdkafka::message::Message;
use rdkafka::producer::Producer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shared::domain::Address;
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{error, info};

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    producer: rdkafka::producer::FutureProducer,
    config: shared::config::AppConfig,
    match_tx: crossbeam::channel::Sender<MatchCommand>,
    ws_tx: broadcast::Sender<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitOrder {
    signed_order: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReconcilePayload {
    user: String,
    deposit: u64,
    block_number: u64,
}

#[derive(Debug, Clone)]
enum MatchCommand {
    PlaceOrder { _order_json: Value },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = shared::config::AppConfig::from_env()?;
    shared::tracing_setup::init(&config.log_format);

    info!("gateway starting");

    let pool = shared::db::init_pool(&config.database_url).await?;
    let producer = shared::kafka::producer(&config.kafka_brokers)?;

    let (match_tx, match_rx) = crossbeam::channel::bounded::<MatchCommand>(10_000);
    let matcher_handle = spawn_matcher(match_rx);

    let (ws_tx, _ws_rx) = broadcast::channel::<Value>(1024);
    let state = AppState {
        pool: pool.clone(),
        producer: producer.clone(),
        config: config.clone(),
        match_tx,
        ws_tx: ws_tx.clone(),
    };

    let ws_consumer = shared::kafka::consumer(
        &config.kafka_brokers,
        "gateway-ws-group",
        &["orders.matched"],
    )?;
    tokio::spawn(ws_broadcast_loop(ws_consumer, ws_tx));

    let app = Router::new()
        .route("/orders", post(post_orders))
        .route("/internal/reconcile", post(post_reconcile))
        .route("/health", get(get_health))
        .route("/ws", get(ws_handler))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(&config.gateway_bind).await?;
    info!("gateway listening on {}", config.gateway_bind);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    drop(state.match_tx);
    let _ = matcher_handle.join();
    info!("gateway shutdown complete");
    Ok(())
}

fn spawn_matcher(rx: crossbeam::channel::Receiver<MatchCommand>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("matcher".to_string())
        .spawn(move || {
            while let Ok(cmd) = rx.recv() {
                info!(?cmd, "matcher received command");
            }
            info!("matcher thread exiting");
        })
        .expect("spawn matcher thread")
}

async fn ws_broadcast_loop(
    consumer: rdkafka::consumer::StreamConsumer,
    ws_tx: broadcast::Sender<Value>,
) {
    let mut stream = consumer.stream();
    while let Some(result) = stream.next().await {
        match result {
            Ok(msg) => {
                if let Some(Ok(payload)) = msg.payload().map(|p| std::str::from_utf8(p)) {
                    if let Ok(val) = serde_json::from_str::<Value>(payload) {
                        let _ = ws_tx.send(val);
                    }
                }
                if let Err(e) = consumer.commit_message(&msg, rdkafka::consumer::CommitMode::Async)
                {
                    error!("kafka commit error: {}", e);
                }
            }
            Err(e) => {
                error!("kafka consumer error: {}", e);
            }
        }
    }
}

async fn post_orders(
    State(state): State<AppState>,
    Json(body): Json<SubmitOrder>,
) -> Result<impl IntoResponse, AppError> {
    let order = &body.signed_order;
    if !order.is_object() {
        return Err(AppError::BadRequest("order must be an object".into()));
    }

    let obj = order.as_object().unwrap();
    if !obj.contains_key("signature") {
        return Err(AppError::BadRequest("missing signature".into()));
    }

    let signature = obj
        .get("signature")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("signature must be string".into()))?;
    let sig_bytes = hex::decode(signature.trim_start_matches("0x"))
        .map_err(|_| AppError::BadRequest("invalid hex signature".into()))?;
    if sig_bytes.len() != 65 {
        return Err(AppError::BadRequest("signature must be 65 bytes".into()));
    }

    let user_str = obj
        .get("maker")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let user = Address::from_hex(user_str)
        .map_err(|_| AppError::BadRequest("invalid maker address".into()))?;

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO holds (user_address, asset_type, amount, status, created_at)
         VALUES ($1, 'usdc', 0, 'held', now())
         ON CONFLICT DO NOTHING",
    )
    .bind(&user.0[..])
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let cmd = MatchCommand::PlaceOrder {
        _order_json: order.clone(),
    };
    state
        .match_tx
        .try_send(cmd)
        .map_err(|e| AppError::Internal(format!("matcher queue full: {}", e)))?;

    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({"status": "accepted" }))))
}

async fn post_reconcile(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ReconcilePayload>,
) -> Result<impl IntoResponse, AppError> {
    let secret = headers
        .get("x-internal-secret")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if secret != state.config.gateway_internal_secret {
        return Err(AppError::Unauthorized);
    }

    let user = Address::from_hex(&body.user)
        .map_err(|_| AppError::BadRequest("invalid user address".into()))?;

    let mut tx = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO users (address) VALUES ($1) ON CONFLICT DO NOTHING",
    )
    .bind(&user.0[..])
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO balances (user_address, asset_type, available_amount, finalized_block_number)
         VALUES ($1, 'usdc', $2, $3)
         ON CONFLICT (user_address, asset_type, COALESCE(position_id, 0))
         DO UPDATE SET available_amount = balances.available_amount + EXCLUDED.available_amount,
                       finalized_block_number = EXCLUDED.finalized_block_number,
                       updated_at = now()",
    )
    .bind(&user.0[..])
    .bind(i64::try_from(body.deposit).unwrap_or(i64::MAX))
    .bind(i64::try_from(body.block_number).unwrap_or(0))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((StatusCode::OK, Json(serde_json::json!({"status": "ok" }))))
}

async fn get_health(State(state): State<AppState>) -> Response {
    let db_ok = sqlx::query("SELECT 1").fetch_one(&state.pool).await.is_ok();

    let kafka_ok = state
        .producer
        .client()
        .fetch_metadata(None, Duration::from_secs(2))
        .is_ok();

    let status = if db_ok && kafka_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let body = serde_json::json!({
        "status": if db_ok && kafka_ok { "ok" } else { "degraded" },
        "db": db_ok,
        "kafka": kafka_ok,
    });
    (status, Json(body)).into_response()
}

async fn ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state.ws_tx.subscribe()))
}

async fn handle_socket(
    mut socket: axum::extract::ws::WebSocket,
    mut rx: broadcast::Receiver<Value>,
) {
    loop {
        match rx.recv().await {
            Ok(msg) => {
                let text = match serde_json::to_string(&msg) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("ws serialize error: {}", e);
                        continue;
                    }
                };
                if socket
                    .send(axum::extract::ws::Message::Text(text))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                break;
            }
        }
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("received SIGTERM, starting graceful shutdown");
}

#[derive(Debug)]
enum AppError {
    BadRequest(String),
    Unauthorized,
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
            AppError::Internal(msg) => {
                error!("internal error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
            }
        };
        let body = Json(serde_json::json!({"error": message}));
        (status, body).into_response()
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Internal(e.to_string())
    }
}
