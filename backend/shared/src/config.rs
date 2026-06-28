use alloy::primitives::Address as AlloyAddress;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub database_url: String,
    pub kafka_brokers: String,
    pub rpc_url: String,
    pub chain_id: u64,
    pub custody_addr: AlloyAddress,
    pub settlement_exchange_addr: AlloyAddress,
    pub oracle_addr: AlloyAddress,
    pub ctf_addr: AlloyAddress,
    pub usdc_addr: AlloyAddress,
    pub operator_key: String,
    pub log_format: String,
    pub gateway_bind: String,
    pub gateway_internal_secret: String,
    #[serde(default = "default_llm_api_url")]
    pub llm_api_url: String,
    #[serde(default = "default_rust_log")]
    pub rust_log: String,
}

fn default_rust_log() -> String {
    "info".to_string()
}

fn default_llm_api_url() -> String {
    "http://localhost:11434/v1/chat/completions".to_string()
}

impl AppConfig {
    pub fn from_env() -> Result<Self, crate::Error> {
        let cfg = config::Config::builder()
            .add_source(config::Environment::default())
            .build()?;
        let config: AppConfig = cfg.try_deserialize()?;
        Ok(config)
    }
}
