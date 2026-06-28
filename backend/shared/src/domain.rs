use alloy::primitives::{keccak256, Address as AlloyAddress, B256, U256};
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

// --- Order types (mirrors Solidity SettlementExchange.Order) ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OrderSide {
    Buy = 0,
    Sell = 1,
}

impl Serialize for OrderSide {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(*self as u8)
    }
}

impl<'de> Deserialize<'de> for OrderSide {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = u8::deserialize(deserializer)?;
        match v {
            0 => Ok(OrderSide::Buy),
            1 => Ok(OrderSide::Sell),
            _ => Err(serde::de::Error::custom("invalid side, expected 0 or 1")),
        }
    }
}

fn deserialize_u256<'de, D>(deserializer: D) -> Result<U256, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        let bytes = hex::decode(hex).map_err(serde::de::Error::custom)?;
        if bytes.len() > 32 {
            return Err(serde::de::Error::custom("u256 exceeds 32 bytes"));
        }
        let mut arr = [0u8; 32];
        arr[32 - bytes.len()..].copy_from_slice(&bytes);
        Ok(U256::from_be_bytes(arr))
    } else {
        s.parse::<U256>().map_err(serde::de::Error::custom)
    }
}

fn deserialize_hex_bytes<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    let hex = s.strip_prefix("0x").unwrap_or(&s);
    hex::decode(hex).map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub salt: Bytes32,
    pub maker: Address,
    pub signer: Address,
    pub condition_id: Bytes32,
    pub parent_collection_id: Bytes32,
    #[serde(deserialize_with = "deserialize_u256")]
    pub position_id: U256,
    pub price: u64,
    pub amount: u64,
    pub side: OrderSide,
    pub nonce: u64,
    pub deadline: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedOrder {
    pub order: Order,
    #[serde(deserialize_with = "deserialize_hex_bytes")]
    pub signature: Vec<u8>,
}

impl Order {
    /// Worst-case USDC collateral for a BUY order (taker, round up).
    /// For SELL orders the collateral is CTF shares = amount.
    pub fn required_collateral(&self) -> u128 {
        let volume = (self.amount as u128 * self.price as u128).div_ceil(1_000_000);
        match self.side {
            OrderSide::Buy => volume,
            OrderSide::Sell => self.amount as u128,
        }
    }

    pub fn asset_type(&self) -> &'static str {
        match self.side {
            OrderSide::Buy => "usdc",
            OrderSide::Sell => "ctf",
        }
    }
}

/// EIP-712 domain separator matching OZ EIP712("Omniscient Exchange", "1").
pub fn compute_domain_separator(chain_id: u64, verifying_contract: AlloyAddress) -> B256 {
    let domain_typehash = keccak256(
        "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
            .as_bytes(),
    );
    let mut encoded = Vec::with_capacity(32 * 5);
    encoded.extend_from_slice(domain_typehash.as_slice());
    encoded.extend_from_slice(keccak256("Omniscient Exchange".as_bytes()).as_slice());
    encoded.extend_from_slice(keccak256("1".as_bytes()).as_slice());
    encoded.extend_from_slice(&U256::from(chain_id).to_be_bytes::<32>());
    encoded.extend_from_slice(&[0u8; 12]);
    encoded.extend_from_slice(verifying_contract.as_slice());
    keccak256(&encoded)
}

/// EIP-712 struct hash for SettlementExchange.Order.
pub fn hash_order(order: &Order) -> B256 {
    let order_typehash = keccak256(
        "Order(bytes32 salt,address maker,address signer,bytes32 conditionId,bytes32 parentCollectionId,uint256 positionId,uint256 price,uint256 amount,uint8 side,uint256 nonce,uint256 deadline)"
            .as_bytes(),
    );
    let mut encoded = Vec::with_capacity(32 * 12);
    encoded.extend_from_slice(order_typehash.as_slice());
    encoded.extend_from_slice(order.salt.0.as_slice());
    encoded.extend_from_slice(&[0u8; 12]);
    encoded.extend_from_slice(order.maker.0.as_slice());
    encoded.extend_from_slice(&[0u8; 12]);
    encoded.extend_from_slice(order.signer.0.as_slice());
    encoded.extend_from_slice(order.condition_id.0.as_slice());
    encoded.extend_from_slice(order.parent_collection_id.0.as_slice());
    encoded.extend_from_slice(&order.position_id.to_be_bytes::<32>());
    encoded.extend_from_slice(&U256::from(order.price).to_be_bytes::<32>());
    encoded.extend_from_slice(&U256::from(order.amount).to_be_bytes::<32>());
    encoded.extend_from_slice(&U256::from(order.side as u8).to_be_bytes::<32>());
    encoded.extend_from_slice(&U256::from(order.nonce).to_be_bytes::<32>());
    encoded.extend_from_slice(&U256::from(order.deadline).to_be_bytes::<32>());
    keccak256(&encoded)
}

pub fn eip712_signing_hash(domain_separator: B256, struct_hash: B256) -> B256 {
    let mut data = Vec::with_capacity(66);
    data.push(0x19);
    data.push(0x01);
    data.extend_from_slice(domain_separator.as_slice());
    data.extend_from_slice(struct_hash.as_slice());
    keccak256(&data)
}

pub fn verify_order_signature(
    order: &Order,
    signature: &[u8],
    chain_id: u64,
    verifying_contract: AlloyAddress,
) -> Result<AlloyAddress, crate::Error> {
    if signature.len() != 65 {
        return Err(crate::Error::Domain(format!(
            "signature must be 65 bytes, got {}",
            signature.len()
        )));
    }
    let domain_separator = compute_domain_separator(chain_id, verifying_contract);
    let struct_hash = hash_order(order);
    let digest = eip712_signing_hash(domain_separator, struct_hash);
    let sig = alloy::primitives::PrimitiveSignature::try_from(signature)
        .map_err(|e| crate::Error::Domain(format!("invalid signature: {e}")))?;
    let recovered = sig
        .recover_address_from_prehash(&digest)
        .map_err(|e| crate::Error::Domain(format!("signature recovery failed: {e}")))?;
    Ok(recovered)
}
