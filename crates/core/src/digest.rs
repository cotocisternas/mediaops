//! Canonical BLAKE3 hex digest. Plan snapshots and title-index rows share this
//! type so a raw 64-char string cannot cross a crate boundary.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 64 lowercase hex characters. Produced by BLAKE3; never a free-form string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Blake3Hex(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DigestError {
    #[error("digest must be 64 lowercase hex characters")]
    Invalid,
}

impl Blake3Hex {
    pub const LEN: usize = 64;

    pub fn parse(raw: &str) -> Result<Self, DigestError> {
        if is_lowercase_b3_hex(raw) {
            Ok(Self(raw.to_string()))
        } else {
            Err(DigestError::Invalid)
        }
    }

    /// Hash `bytes` with BLAKE3. The result is always a valid digest.
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(blake3::hash(bytes).to_hex().to_string())
    }

    /// Hash a reader. Does not touch the filesystem itself.
    pub fn of_reader(mut reader: impl std::io::Read) -> Result<Self, std::io::Error> {
        let mut hasher = blake3::Hasher::new();
        let mut buf = [0_u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(Self(hasher.finalize().to_hex().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Blake3Hex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Blake3Hex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Blake3Hex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Blake3Hex::parse(&raw).map_err(serde::de::Error::custom)
    }
}

fn is_lowercase_b3_hex(s: &str) -> bool {
    s.len() == Blake3Hex::LEN && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn parse_rejects_short_and_uppercase() {
        assert_eq!(Blake3Hex::parse("abc"), Err(DigestError::Invalid));
        assert_eq!(
            Blake3Hex::parse(&A.to_ascii_uppercase()),
            Err(DigestError::Invalid)
        );
        assert_eq!(Blake3Hex::parse(A).expect("ok").as_str(), A);
    }

    #[test]
    fn of_bytes_is_lowercase_hex() {
        let digest = Blake3Hex::of_bytes(b"hello");
        assert_eq!(digest.as_str().len(), Blake3Hex::LEN);
        assert!(is_lowercase_b3_hex(digest.as_str()));
        assert_eq!(
            digest,
            Blake3Hex::parse(digest.as_str()).expect("round-trip")
        );
    }
}
