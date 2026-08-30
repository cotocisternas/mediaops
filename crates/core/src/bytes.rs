//! Size values that cross a crate boundary. Display and serde are raw byte counts.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A size in bytes. Config fields named `*_gib` / `*_mib` become this at parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bytes(u64);

impl Bytes {
    pub const GIB: u64 = 1 << 30;
    pub const MIB: u64 = 1 << 20;

    pub const fn new(n: u64) -> Self {
        Self(n)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_serde_are_raw_byte_counts() {
        let bytes = Bytes::new(256 * Bytes::GIB);
        assert_eq!(bytes.to_string(), "274877906944");
        let encoded = serde_json::to_string(&bytes).expect("serialize");
        assert_eq!(encoded, "274877906944");
        let decoded: Bytes = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, bytes);
        assert_eq!(decoded.get(), 256 * Bytes::GIB);
    }
}
