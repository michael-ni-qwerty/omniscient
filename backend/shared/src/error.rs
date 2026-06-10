use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("kafka error: {0}")]
    Kafka(#[from] rdkafka::error::KafkaError),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("domain error: {0}")]
    Domain(String),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("alloy error: {0}")]
    Alloy(String),
}

impl From<config::ConfigError> for Error {
    fn from(e: config::ConfigError) -> Self {
        Error::Config(e.to_string())
    }
}
