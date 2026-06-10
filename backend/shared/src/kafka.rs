use rdkafka::config::ClientConfig;
use rdkafka::consumer::{Consumer, StreamConsumer};
use rdkafka::producer::FutureProducer;
use serde_json::Value;

pub fn producer(brokers: &str) -> Result<FutureProducer, crate::Error> {
    let p: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", brokers)
        .set("queue.buffering.max.ms", "10")
        .set("message.timeout.ms", "5000")
        .create()
        .map_err(crate::Error::Kafka)?;
    Ok(p)
}

pub fn consumer(
    brokers: &str,
    group_id: &str,
    topics: &[&str],
) -> Result<StreamConsumer, crate::Error> {
    let c: StreamConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers)
        .set("group.id", group_id)
        .set("auto.offset.reset", "earliest")
        .set("enable.auto.commit", "false")
        .create()
        .map_err(crate::Error::Kafka)?;
    c.subscribe(topics).map_err(crate::Error::Kafka)?;
    Ok(c)
}

pub fn payload_with_version(data: Value) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("schema_version".to_string(), serde_json::json!(1u16));
    if let Value::Object(mut map) = data {
        obj.append(&mut map);
    } else {
        obj.insert("payload".to_string(), data);
    }
    Value::Object(obj)
}
