use alloy::primitives::{keccak256, B256, U256};
use alloy::providers::ProviderBuilder;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shared::domain::MarketId;
use sqlx::PgPool;
use std::time::Duration;
use tokio::time::{interval, sleep};
use tracing::{error as log_error, info, warn};

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ResolutionSpec {
    resolver_id: String,
    params: Value,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct ExpiredMarket {
    market_id: MarketId,
    resolver_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResolutionProposal {
    outcome: u8,
    payouts: Vec<u64>,
    evidence: Vec<EvidenceItem>,
    confidence: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EvidenceItem {
    url: String,
    verified: bool,
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
enum SpecError {
    #[error("invalid spec: {0}")]
    Invalid(String),
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
enum ResolveError {
    #[error("fallback to human/DAO")]
    Fallback,
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("parse error: {0}")]
    Parse(String),
}

#[async_trait]
#[allow(dead_code)]
trait Resolver: Send + Sync {
    fn id(&self) -> &'static str;
    async fn validate_spec(&self, spec: &ResolutionSpec) -> Result<(), SpecError>;
    async fn resolve(&self, market: &ExpiredMarket) -> Result<ResolutionProposal, ResolveError>;
}

struct AiResolver {
    api_url: String,
    api_key: String,
}

#[async_trait]
impl Resolver for AiResolver {
    fn id(&self) -> &'static str {
        "ai"
    }

    async fn validate_spec(&self, spec: &ResolutionSpec) -> Result<(), SpecError> {
        if spec.resolver_id != "ai" {
            return Err(SpecError::Invalid("resolver_id must be 'ai'".into()));
        }
        Ok(())
    }

    async fn resolve(&self, _market: &ExpiredMarket) -> Result<ResolutionProposal, ResolveError> {
        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Resolve this market."}],
            "temperature": 0,
        });

        let response = client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .timeout(Duration::from_secs(30))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ResolveError::Fallback);
        }

        let json: Value = response.json().await?;
        let content = json
            .get("choices")
            .and_then(|v| v.get(0))
            .and_then(|v| v.get("message"))
            .and_then(|v| v.get("content"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let parsed: Value = serde_json::from_str(content).unwrap_or_else(|_| {
            serde_json::json!({
                "outcome": 0,
                "confidence": 0,
                "evidence": []
            })
        });

        let confidence = parsed
            .get("confidence")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u8;

        if confidence < 70 {
            return Err(ResolveError::Fallback);
        }

        let evidence_urls: Vec<String> = parsed
            .get("evidence")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.get("url").and_then(|u| u.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        for url in &evidence_urls {
            let check = client.get(url).send().await;
            if let Ok(resp) = check {
                if !resp.status().is_success() {
                    return Err(ResolveError::Fallback);
                }
            } else {
                return Err(ResolveError::Fallback);
            }
        }

        let outcome = parsed
            .get("outcome")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u8;
        let payouts = vec![100 - outcome as u64, outcome as u64];

        Ok(ResolutionProposal {
            outcome,
            payouts,
            evidence: evidence_urls
                .into_iter()
                .map(|url| EvidenceItem { url, verified: true })
                .collect(),
            confidence,
        })
    }
}

struct ApiResolver;

#[async_trait]
impl Resolver for ApiResolver {
    fn id(&self) -> &'static str {
        "api"
    }

    async fn validate_spec(&self, spec: &ResolutionSpec) -> Result<(), SpecError> {
        if !spec.resolver_id.starts_with("api.") {
            return Err(SpecError::Invalid("resolver_id must start with 'api.'".into()));
        }
        Ok(())
    }

    async fn resolve(&self, _market: &ExpiredMarket) -> Result<ResolutionProposal, ResolveError> {
        let client = reqwest::Client::new();
        let response = client
            .get("https://api.example.com/result")
            .timeout(Duration::from_secs(30))
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(ResolveError::Fallback);
        }

        let json: Value = response.json().await?;
        let outcome = json.get("outcome").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
        let confidence = json.get("confidence").and_then(|v| v.as_u64()).unwrap_or(100) as u8;
        let payouts = vec![100 - outcome as u64, outcome as u64];

        Ok(ResolutionProposal {
            outcome,
            payouts,
            evidence: vec![],
            confidence,
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let config = shared::config::AppConfig::from_env()?;
    shared::tracing_setup::init(&config.log_format);
    info!("resolution service starting");

    let pool = shared::db::init_pool(&config.database_url).await?;
    let _provider = ProviderBuilder::new().on_http(config.rpc_url.parse()?);

    let ai = AiResolver {
        api_url: config.llm_api_url.clone(),
        api_key: config.llm_api_key.clone(),
    };
    let api = ApiResolver;

    let pool2 = pool.clone();
    let expiry_handle = tokio::spawn(async move {
        expiry_watcher(pool2, &ai, &api).await;
    });

    let pool3 = pool.clone();
    let reveal_handle = tokio::spawn(async move {
        commit_reveal_loop(pool3).await;
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("received SIGTERM, shutting down resolution service");
        }
        _ = expiry_handle => {}
        _ = reveal_handle => {}
    }

    Ok(())
}

async fn expiry_watcher(pool: PgPool, ai: &AiResolver, api: &ApiResolver) {
    let mut ticker = interval(Duration::from_secs(10));
    loop {
        ticker.tick().await;

        let rows = match sqlx::query_as::<_, (Vec<u8>, String)>(
            "SELECT market_id, resolver_id FROM markets
             WHERE state = 'open' AND expiry <= now()",
        )
        .fetch_all(&pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                log_error!("expiry query error: {}", e);
                continue;
            }
        };

        for (market_id_bytes, resolver_id) in rows {
            let market_id = match MarketId::try_from(&market_id_bytes[..]) {
                Ok(m) => m,
                Err(e) => {
                    log_error!("invalid market_id: {}", e);
                    continue;
                }
            };

            let mut tx = match pool.begin().await {
                Ok(t) => t,
                Err(e) => {
                    log_error!("tx begin error: {}", e);
                    continue;
                }
            };

            let updated = sqlx::query(
                "UPDATE markets SET state = 'expired', updated_at = now()
                 WHERE market_id = $1 AND state = 'open'",
            )
            .bind(&market_id_bytes[..])
            .execute(&mut *tx)
            .await;

            if let Err(e) = updated {
                log_error!("update market error: {}", e);
                continue;
            }

            let _spec = ResolutionSpec {
                resolver_id: resolver_id.clone(),
                params: Value::Null,
            };

            let market = ExpiredMarket {
                market_id,
                resolver_id: resolver_id.clone(),
            };

            let proposal = if resolver_id == "ai" {
                ai.resolve(&market).await
            } else if resolver_id.starts_with("api.") {
                api.resolve(&market).await
            } else {
                Err(ResolveError::Fallback)
            };

            match proposal {
                Ok(p) => {
                    let commitment = compute_commitment(&market_id, &p.payouts);
                    let _ = sqlx::query(
                        "INSERT INTO resolution_proposals
                         (market_id, resolver_id, commitment, proposed_payouts, confidence, evidence, status, created_at, updated_at)
                         VALUES ($1, $2, $3, $4, $5, $6, 'pending', now(), now())
                         ON CONFLICT (market_id) DO UPDATE SET
                         commitment = EXCLUDED.commitment,
                         proposed_payouts = EXCLUDED.proposed_payouts,
                         confidence = EXCLUDED.confidence,
                         evidence = EXCLUDED.evidence,
                         status = 'pending',
                         updated_at = now()",
                    )
                    .bind(&market_id_bytes[..])
                    .bind(&resolver_id)
                    .bind(commitment.as_slice())
                    .bind(&p.payouts.iter().map(|v| *v as i64).collect::<Vec<i64>>()[..])
                    .bind(i32::from(p.confidence))
                    .bind(serde_json::to_value(&p.evidence).unwrap_or(Value::Null))
                    .execute(&mut *tx)
                    .await;
                }
                Err(e) => {
                    warn!("resolution fallback for {}: {}", market_id, e);
                    let _ = sqlx::query(
                        "INSERT INTO resolution_proposals
                         (market_id, resolver_id, status, created_at, updated_at)
                         VALUES ($1, $2, 'pending', now(), now())
                         ON CONFLICT (market_id) DO NOTHING",
                    )
                    .bind(&market_id_bytes[..])
                    .bind(&resolver_id)
                    .execute(&mut *tx)
                    .await;
                }
            }

            if let Err(e) = tx.commit().await {
                log_error!("tx commit error: {}", e);
            }
        }
    }
}

async fn commit_reveal_loop(pool: PgPool) {
    let mut ticker = interval(Duration::from_secs(30));
    loop {
        ticker.tick().await;

        let rows = match sqlx::query_as::<_, (Vec<u8>, Vec<u8>, Option<Vec<i64>>)>(
            "SELECT market_id, commitment, proposed_payouts
             FROM resolution_proposals
             WHERE status = 'pending' AND commitment IS NOT NULL",
        )
        .fetch_all(&pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                log_error!("commit query error: {}", e);
                continue;
            }
        };

        for (market_id_bytes, _commitment, _payouts_opt) in rows {
            info!(
                "would commit outcome for market {}",
                hex::encode(&market_id_bytes)
            );

            let mut tx = match pool.begin().await {
                Ok(t) => t,
                Err(e) => {
                    log_error!("tx begin error: {}", e);
                    continue;
                }
            };

            let _ = sqlx::query(
                "UPDATE resolution_proposals
                 SET status = 'committed', committed_at = now()
                 WHERE market_id = $1",
            )
            .bind(&market_id_bytes)
            .execute(&mut *tx)
            .await;

            if let Err(e) = tx.commit().await {
                log_error!("tx commit error: {}", e);
                continue;
            }

            sleep(Duration::from_secs(300)).await;

            let mut tx = match pool.begin().await {
                Ok(t) => t,
                Err(e) => {
                    log_error!("tx begin error: {}", e);
                    continue;
                }
            };

            let _ = sqlx::query(
                "UPDATE resolution_proposals
                 SET status = 'revealed', revealed_at = now()
                 WHERE market_id = $1",
            )
            .bind(&market_id_bytes)
            .execute(&mut *tx)
            .await;

            if let Err(e) = tx.commit().await {
                log_error!("tx commit error: {}", e);
                continue;
            }

            sleep(Duration::from_secs(3600)).await;

            let mut tx = match pool.begin().await {
                Ok(t) => t,
                Err(e) => {
                    log_error!("tx begin error: {}", e);
                    continue;
                }
            };

            let _ = sqlx::query(
                "UPDATE resolution_proposals
                 SET status = 'resolved', finalized_at = now()
                 WHERE market_id = $1",
            )
            .bind(&market_id_bytes)
            .execute(&mut *tx)
            .await;

            let _ = sqlx::query(
                "UPDATE markets SET state = 'resolved', updated_at = now()
                 WHERE market_id = $1",
            )
            .bind(&market_id_bytes)
            .execute(&mut *tx)
            .await;

            if let Err(e) = tx.commit().await {
                log_error!("tx commit error: {}", e);
            }
        }
    }
}

fn compute_commitment(market_id: &MarketId, payouts: &[u64]) -> B256 {
    let mut data = Vec::new();
    data.extend_from_slice(&market_id.0);
    for p in payouts {
        data.extend_from_slice(&U256::from(*p).to_be_bytes::<32>());
    }
    keccak256(&data)
}
