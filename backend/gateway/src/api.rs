use crate::service::{self, AppState};
use axum::{
    extract::{Path, State, WebSocketUpgrade},
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
use shared::domain::{MarketId, OrderId, SignedOrder};
use tracing::error;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/orders", post(post_orders))
        .route("/orders/:order_id/cancel", post(post_cancel_order))
        .route("/internal/reconcile", post(post_reconcile))
        .route("/health", get(get_health))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitOrderRequest {
    market_id: String,
    signed_order: SignedOrder,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubmitOrderResponse {
    order_id: OrderId,
    status: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelOrderRequest {
    position_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReconcilePayload {
    user: String,
    deposit: u64,
    block_number: u64,
}

async fn post_orders(
    State(state): State<AppState>,
    Json(body): Json<SubmitOrderRequest>,
) -> Result<impl IntoResponse, AppError> {
    let market_id = MarketId::from_hex(&body.market_id)
        .map_err(|e| AppError::BadRequest(format!("invalid market_id: {e}")))?;

    let order_id = service::accept_order(&state, &body.signed_order, &market_id)
        .await
        .map_err(AppError::from_order_error)?;

    Ok((
        StatusCode::ACCEPTED,
        Json(SubmitOrderResponse {
            order_id,
            status: "accepted",
        }),
    ))
}

async fn post_cancel_order(
    State(state): State<AppState>,
    Path(order_id): Path<String>,
    Json(body): Json<CancelOrderRequest>,
) -> Result<impl IntoResponse, AppError> {
    let order_id = OrderId(
        uuid::Uuid::parse_str(&order_id)
            .map_err(|e| AppError::BadRequest(format!("invalid order_id: {e}")))?,
    );

    let position_id: alloy::primitives::U256 = body
        .position_id
        .parse()
        .map_err(|e| AppError::BadRequest(format!("invalid position_id: {e}")))?;

    service::cancel_order(&state, order_id, &position_id)
        .await
        .map_err(AppError::from_order_error)?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({"status": "cancelled"})),
    ))
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

    let user = shared::domain::Address::from_hex(&body.user)
        .map_err(|e| AppError::BadRequest(format!("invalid user address: {e}")))?;

    let mut tx = state.pool.begin().await.map_err(|e| {
        error!("tx begin error: {}", e);
        AppError::Internal("database error".into())
    })?;

    sqlx::query("INSERT INTO users (address) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(&user.0[..])
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            error!("user insert error: {}", e);
            AppError::Internal("database error".into())
        })?;

    sqlx::query(
        "INSERT INTO balances (user_address, asset_type, available_amount, finalized_block_number)
         VALUES ($1, 'usdc', $2, $3)
         ON CONFLICT (user_address, asset_type, COALESCE(position_id, -1))
         DO UPDATE SET available_amount = balances.available_amount + EXCLUDED.available_amount,
                       finalized_block_number = EXCLUDED.finalized_block_number,
                       updated_at = now()",
    )
    .bind(&user.0[..])
    .bind(i64::try_from(body.deposit).unwrap_or(i64::MAX))
    .bind(i64::try_from(body.block_number).unwrap_or(0))
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        error!("balance upsert error: {}", e);
        AppError::Internal("database error".into())
    })?;

    tx.commit().await.map_err(|e| {
        error!("tx commit error: {}", e);
        AppError::Internal("database error".into())
    })?;

    Ok((StatusCode::OK, Json(serde_json::json!({"status": "ok"}))))
}

async fn get_health(State(state): State<AppState>) -> Response {
    let db_ok = sqlx::query("SELECT 1").fetch_one(&state.pool).await.is_ok();

    let kafka_ok = state
        .producer
        .client()
        .fetch_metadata(None, std::time::Duration::from_secs(2))
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

async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state.ws_tx.subscribe()))
}

async fn handle_socket(
    mut socket: axum::extract::ws::WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<serde_json::Value>,
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
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                break;
            }
        }
    }
}

#[derive(Debug)]
pub enum AppError {
    BadRequest(String),
    Unauthorized,
    Internal(String),
}

impl AppError {
    fn from_order_error(e: String) -> Self {
        if e.contains("insufficient") || e.contains("expired") || e.contains("invalid") {
            AppError::BadRequest(e)
        } else {
            AppError::Internal(e)
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".into()),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(serde_json::json!({"error": msg}))).into_response()
    }
}

/// WS broadcast loop: consume from Redpanda `orders.matched` and broadcast to WS clients.
pub async fn ws_broadcast_loop(
    consumer: rdkafka::consumer::StreamConsumer,
    ws_tx: tokio::sync::broadcast::Sender<serde_json::Value>,
) {
    let mut stream = consumer.stream();
    while let Some(result) = stream.next().await {
        match result {
            Ok(msg) => {
                if let Some(Ok(payload)) = msg.payload().map(|p| std::str::from_utf8(p)) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(payload) {
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
