use alloy::primitives::{keccak256, B256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::network::{EthereumWallet, TransactionBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use chrono::Utc;
use futures::StreamExt;
use rdkafka::consumer::Consumer;
use rdkafka::message::Message;
use serde::{Deserialize, Serialize};
// use shared::domain::MarketId;
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::{interval, Instant};
use tracing::{error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MatchEvent {
    market_id: String,
    maker: String,
    taker: String,
    price: u64,
    amount: u64,
    side: String,
}

#[derive(Default)]
struct BatchState {
    deltas: BTreeMap<(String, String), i128>,
    events: Vec<MatchEvent>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = shared::config::AppConfig::from_env()?;
    shared::tracing_setup::init(&config.log_format);
    info!("settlement service starting");

    let pool = shared::db::init_pool(&config.database_url).await?;
    let consumer = shared::kafka::consumer(
        &config.kafka_brokers,
        "settlement-group",
        &["orders.matched"],
    )?;

    let provider = ProviderBuilder::new().on_http(config.rpc_url.parse()?);

    batch_loop(consumer, pool, provider, config).await;
    Ok(())
}

async fn batch_loop(
    consumer: rdkafka::consumer::StreamConsumer,
    pool: PgPool,
    provider: alloy::providers::RootProvider<alloy::transports::http::Http<reqwest::Client>>,
    config: shared::config::AppConfig,
) {
    let mut stream = consumer.stream();
    let mut batch: BatchState = BatchState::default();
    let mut last_batch_time = Instant::now();
    let mut ticker = interval(Duration::from_secs(5));
    let batch_size_threshold: usize = 50;

    loop {
        tokio::select! {
            msg = stream.next() => {
                if let Some(result) = msg {
                    match result {
                        Ok(msg) => {
                            if let Some(Ok(payload)) = msg.payload().map(|p| std::str::from_utf8(p)) {
                                match serde_json::from_str::<MatchEvent>(payload) {
                                    Ok(event) => {
                                        let key = (event.maker.clone(), "usdc".to_string());
                                        *batch.deltas.entry(key).or_insert(0) += event.amount as i128;
                                        batch.events.push(event);
                                    }
                                    Err(e) => {
                                        warn!("failed to deserialize match event: {}", e);
                                    }
                                }
                            }
                            if batch.events.len() >= batch_size_threshold {
                                if let Err(e) = submit_batch(&pool, &provider, &config, &mut batch, &consumer, &msg).await {
                                    error!("batch submission failed: {}", e);
                                }
                                last_batch_time = Instant::now();
                            }
                        }
                        Err(e) => {
                            error!("kafka error: {}", e);
                        }
                    }
                }
            }
            _ = ticker.tick() => {
                if !batch.events.is_empty() && last_batch_time.elapsed() >= Duration::from_secs(5) {
                    if let Err(e) = submit_batch_timed(&pool, &provider, &config, &mut batch).await {
                        error!("timed batch submission failed: {}", e);
                    }
                    last_batch_time = Instant::now();
                }
            }
        }
    }
}

async fn submit_batch(
    pool: &PgPool,
    provider: &alloy::providers::RootProvider<alloy::transports::http::Http<reqwest::Client>>,
    config: &shared::config::AppConfig,
    batch: &mut BatchState,
    consumer: &rdkafka::consumer::StreamConsumer,
    msg: &rdkafka::message::BorrowedMessage<'_>,
) -> Result<(), shared::Error> {
    let batch_id = compute_batch_id(batch);
    let batch_id_bytes: [u8; 32] = batch_id.into();

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO settlement_batches (batch_id, status, created_at)
         VALUES ($1, 'pending', now())
         ON CONFLICT (batch_id) DO NOTHING",
    )
    .bind(&batch_id_bytes[..])
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    info!("submitting batch {}", hex::encode(batch_id_bytes));

    let signer: PrivateKeySigner = config
        .operator_key
        .parse()
        .map_err(|e| shared::Error::Alloy(format!("{e}")))?;

    let tx_request = TransactionRequest::default()
        .to(config.settlement_exchange_addr)
        .input(batch_id_bytes.to_vec().into());

    let wallet = EthereumWallet::from(signer);

    let tx_envelope = tx_request
        .build(&wallet)
        .await
        .map_err(|e| shared::Error::Alloy(format!("{e}")))?;

    let receipt = provider
        .send_tx_envelope(tx_envelope)
        .await
        .map_err(|e| shared::Error::Alloy(format!("{e}")))?;

    let tx_hash = receipt.tx_hash();
    let submitted_at = Utc::now();

    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE settlement_batches
         SET status = 'submitted', tx_hash = $1, nonce = $2, submitted_at = $3
         WHERE batch_id = $4",
    )
    .bind(tx_hash.as_slice())
    .bind(submitted_at)
    .bind(&batch_id_bytes[..])
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    wait_for_finality(provider, *tx_hash).await;

    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE settlement_batches
         SET status = 'finalized', finalized_at = now()
         WHERE batch_id = $1",
    )
    .bind(&batch_id_bytes[..])
    .execute(&mut *tx)
    .await?;

    for event in &batch.events {
        let maker_bytes = hex::decode(event.maker.trim_start_matches("0x"))
            .unwrap_or_default();
        sqlx::query(
            "UPDATE orders SET filled_amount = filled_amount + $1, status = 'filled'
             WHERE user_address = $2 AND status = 'open'",
        )
        .bind(i64::try_from(event.amount).unwrap_or(0))
        .bind(maker_bytes)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    if let Err(e) = consumer.commit_message(msg, rdkafka::consumer::CommitMode::Async) {
        error!("kafka commit error: {}", e);
    }

    batch.events.clear();
    batch.deltas.clear();

    Ok(())
}

async fn submit_batch_timed(
    pool: &PgPool,
    provider: &alloy::providers::RootProvider<alloy::transports::http::Http<reqwest::Client>>,
    config: &shared::config::AppConfig,
    batch: &mut BatchState,
) -> Result<(), shared::Error> {
    if batch.events.is_empty() {
        return Ok(());
    }
    let batch_id = compute_batch_id(batch);
    let batch_id_bytes: [u8; 32] = batch_id.into();

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO settlement_batches (batch_id, status, created_at)
         VALUES ($1, 'pending', now())
         ON CONFLICT (batch_id) DO NOTHING",
    )
    .bind(&batch_id_bytes[..])
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    info!("submitting timed batch {}", hex::encode(batch_id_bytes));

    let signer: PrivateKeySigner = config
        .operator_key
        .parse()
        .map_err(|e| shared::Error::Alloy(format!("{e}")))?;

    let tx_request = TransactionRequest::default()
        .to(config.settlement_exchange_addr)
        .input(batch_id_bytes.to_vec().into());

    let wallet = EthereumWallet::from(signer);

    let tx_envelope = tx_request
        .build(&wallet)
        .await
        .map_err(|e| shared::Error::Alloy(format!("{e}")))?;

    let receipt = provider
        .send_tx_envelope(tx_envelope)
        .await
        .map_err(|e| shared::Error::Alloy(format!("{e}")))?;

    let tx_hash = receipt.tx_hash();
    let submitted_at = Utc::now();

    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE settlement_batches
         SET status = 'submitted', tx_hash = $1, submitted_at = $2
         WHERE batch_id = $3",
    )
    .bind(tx_hash.as_slice())
    .bind(submitted_at)
    .bind(&batch_id_bytes[..])
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    wait_for_finality(provider, *tx_hash).await;

    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE settlement_batches
         SET status = 'finalized', finalized_at = now()
         WHERE batch_id = $1",
    )
    .bind(&batch_id_bytes[..])
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    batch.events.clear();
    batch.deltas.clear();

    Ok(())
}

fn compute_batch_id(batch: &BatchState) -> B256 {
    let data = serde_json::to_vec(&batch.deltas).unwrap_or_default();
    keccak256(&data)
}

async fn wait_for_finality(
    provider: &alloy::providers::RootProvider<alloy::transports::http::Http<reqwest::Client>>,
    tx_hash: B256,
) {
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        match provider.get_transaction_receipt(tx_hash).await {
            Ok(Some(receipt)) => {
                if let Some(block_number) = receipt.block_number {
                    match provider.get_block_number().await {
                        Ok(current) => {
                            if current.saturating_sub(block_number)
                                >= shared::constants::FINALITY_BLOCKS
                            {
                                info!("tx {} finalized", tx_hash);
                                return;
                            }
                        }
                        Err(e) => {
                            warn!("get_block_number error: {}", e);
                        }
                    }
                }
            }
            Ok(None) => {
                warn!("receipt not yet available for {}", tx_hash);
            }
            Err(e) => {
                warn!("get_transaction_receipt error: {}", e);
            }
        }
    }
    warn!("finality wait timeout for {}", tx_hash);
}
