use alloy::primitives::U256;
use bigdecimal::BigDecimal;
use std::str::FromStr;

pub fn topic_hash(sig: &[u8]) -> alloy::primitives::B256 {
    alloy::primitives::keccak256(sig)
}

pub fn u256_to_bigdecimal(u: U256) -> BigDecimal {
    BigDecimal::from_str(&u.to_string())
        .unwrap_or_else(|_| panic!("U256 -> BigDecimal conversion failed for {}", u))
}

pub fn parse_u256_array(data: &[u8]) -> Option<Vec<BigDecimal>> {
    if data.len() < 64 {
        return None;
    }
    let off: [u8; 32] = data[0..32].try_into().ok()?;
    let offset = U256::from_be_bytes(off).to::<usize>();
    if data.len() < offset + 32 {
        return None;
    }
    let len_bytes: [u8; 32] = data[offset..offset + 32].try_into().ok()?;
    let len = U256::from_be_bytes(len_bytes).to::<usize>();
    let elems_start = offset + 32;
    if data.len() < elems_start + len * 32 {
        return None;
    }
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let start = elems_start + i * 32;
        let val_bytes: [u8; 32] = data[start..start + 32].try_into().ok()?;
        out.push(u256_to_bigdecimal(U256::from_be_bytes(val_bytes)));
    }
    Some(out)
}

pub fn parse_string(data: &[u8]) -> Option<String> {
    if data.len() < 64 {
        return None;
    }
    let off: [u8; 32] = data[0..32].try_into().ok()?;
    let str_offset = U256::from_be_bytes(off).to::<usize>();
    if data.len() < str_offset + 32 {
        return None;
    }
    let len_bytes: [u8; 32] = data[str_offset..str_offset + 32].try_into().ok()?;
    let len = U256::from_be_bytes(len_bytes).to::<usize>();
    if data.len() < str_offset + 32 + len {
        return None;
    }
    String::from_utf8(data[str_offset + 32..str_offset + 32 + len].to_vec()).ok()
}
