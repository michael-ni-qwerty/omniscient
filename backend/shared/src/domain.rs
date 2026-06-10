use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address(pub [u8; 20]);

impl Address {
    pub fn from_hex(s: &str) -> Result<Self, crate::Error> {
        let stripped = s.trim().strip_prefix("0x").unwrap_or(s);
        if stripped.len() != 40 {
            return Err(crate::Error::Domain(format!(
                "address must be 40 hex chars, got {}",
                stripped.len()
            )));
        }
        let mut bytes = [0u8; 20];
        hex::decode_to_slice(stripped, &mut bytes)
            .map_err(|e| crate::Error::Domain(format!("hex decode: {e}")))?;
        Ok(Address(bytes))
    }
}

impl TryFrom<&[u8]> for Address {
    type Error = crate::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() != 20 {
            return Err(crate::Error::Domain(format!(
                "address must be 20 bytes, got {}",
                value.len()
            )));
        }
        let mut arr = [0u8; 20];
        arr.copy_from_slice(value);
        Ok(Address(arr))
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", hex::encode(self.0))
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({})", self)
    }
}

impl Serialize for Address {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MarketId(pub [u8; 32]);

impl MarketId {
    pub fn from_hex(s: &str) -> Result<Self, crate::Error> {
        let stripped = s.trim().strip_prefix("0x").unwrap_or(s);
        if stripped.len() != 64 {
            return Err(crate::Error::Domain(format!(
                "market_id must be 64 hex chars, got {}",
                stripped.len()
            )));
        }
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(stripped, &mut bytes)
            .map_err(|e| crate::Error::Domain(format!("hex decode: {e}")))?;
        Ok(MarketId(bytes))
    }
}

impl TryFrom<&[u8]> for MarketId {
    type Error = crate::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() != 32 {
            return Err(crate::Error::Domain(format!(
                "market_id must be 32 bytes, got {}",
                value.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(value);
        Ok(MarketId(arr))
    }
}

impl fmt::Display for MarketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", hex::encode(self.0))
    }
}

impl fmt::Debug for MarketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MarketId({})", self)
    }
}

impl Serialize for MarketId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MarketId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bytes32(pub [u8; 32]);

impl Bytes32 {
    pub fn from_hex(s: &str) -> Result<Self, crate::Error> {
        let stripped = s.trim().strip_prefix("0x").unwrap_or(s);
        if stripped.len() != 64 {
            return Err(crate::Error::Domain(format!(
                "bytes32 must be 64 hex chars, got {}",
                stripped.len()
            )));
        }
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(stripped, &mut bytes)
            .map_err(|e| crate::Error::Domain(format!("hex decode: {e}")))?;
        Ok(Bytes32(bytes))
    }
}

impl TryFrom<&[u8]> for Bytes32 {
    type Error = crate::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() != 32 {
            return Err(crate::Error::Domain(format!(
                "bytes32 must be 32 bytes, got {}",
                value.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(value);
        Ok(Bytes32(arr))
    }
}

impl fmt::Display for Bytes32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", hex::encode(self.0))
    }
}

impl fmt::Debug for Bytes32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes32({})", self)
    }
}

impl Serialize for Bytes32 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Bytes32 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Price(pub u64);

impl Price {
    pub const SCALE: u64 = 1_000_000;
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Price({})", self.0)
    }
}

impl Serialize for Price {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for Price {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = u64::deserialize(deserializer)?;
        Ok(Price(v))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Quantity(pub u64);

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Quantity({})", self.0)
    }
}

impl Serialize for Quantity {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for Quantity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = u64::deserialize(deserializer)?;
        Ok(Quantity(v))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrderId(pub Uuid);

impl OrderId {
    pub fn new() -> Self {
        OrderId(Uuid::new_v4())
    }
}

impl Default for OrderId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for OrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for OrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OrderId({})", self.0)
    }
}

impl Serialize for OrderId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for OrderId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let uuid = Uuid::parse_str(&s).map_err(serde::de::Error::custom)?;
        Ok(OrderId(uuid))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BatchId(pub [u8; 32]);

impl BatchId {
    pub fn from_hex(s: &str) -> Result<Self, crate::Error> {
        let stripped = s.trim().strip_prefix("0x").unwrap_or(s);
        if stripped.len() != 64 {
            return Err(crate::Error::Domain(format!(
                "batch_id must be 64 hex chars, got {}",
                stripped.len()
            )));
        }
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(stripped, &mut bytes)
            .map_err(|e| crate::Error::Domain(format!("hex decode: {e}")))?;
        Ok(BatchId(bytes))
    }
}

impl TryFrom<&[u8]> for BatchId {
    type Error = crate::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() != 32 {
            return Err(crate::Error::Domain(format!(
                "batch_id must be 32 bytes, got {}",
                value.len()
            )));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(value);
        Ok(BatchId(arr))
    }
}

impl fmt::Display for BatchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{}", hex::encode(self.0))
    }
}

impl fmt::Debug for BatchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BatchId({})", self)
    }
}

impl Serialize for BatchId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for BatchId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}
