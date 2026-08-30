//! Dual-digest title index (AD-8). Pure types and the repository port.
//!
//! `install_b3` is the reclaim/local-proof digest, written once by
//! [`TitleIndexRepo::record_install`] after [`crate::install::install`].
//! `current_b3` is what `verify` checks: that same call sets it, and only
//! [`TitleIndexRepo::record_replace`] (encode's [`crate::install::replace`])
//! updates it afterwards.

use crate::title_id::TitleId;

/// One `title_index` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleIndexEntry {
    title_id: TitleId,
    install_b3: String,
    current_b3: String,
}

impl TitleIndexEntry {
    pub fn new(
        title_id: TitleId,
        install_b3: impl Into<String>,
        current_b3: impl Into<String>,
    ) -> Result<Self, TitleIndexError> {
        let install_b3 = install_b3.into();
        let current_b3 = current_b3.into();
        if !is_lowercase_b3_hex(&install_b3) || !is_lowercase_b3_hex(&current_b3) {
            return Err(TitleIndexError::InvalidDigest);
        }
        Ok(Self {
            title_id,
            install_b3,
            current_b3,
        })
    }

    pub fn title_id(&self) -> &TitleId {
        &self.title_id
    }

    pub fn install_b3(&self) -> &str {
        &self.install_b3
    }

    pub fn current_b3(&self) -> &str {
        &self.current_b3
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TitleIndexError {
    #[error("digest must be 64 lowercase hex characters")]
    InvalidDigest,
    #[error("install_b3 is immutable")]
    InstallDigestImmutable,
    #[error("no title_index row to replace")]
    NotInstalled,
}

pub(crate) fn is_lowercase_b3_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Persistence door for the install gate. Adapter lives in `store`.
///
/// A trait, not I/O: async signatures only.
#[allow(async_fn_in_trait)]
pub trait TitleIndexRepo: Send + Sync {
    type Error;

    async fn get(&self, title_id: &TitleId) -> Result<Option<TitleIndexEntry>, Self::Error>;
    /// First placement: writes `install_b3` and `current_b3`. `install_b3` never changes.
    async fn record_install(&self, title_id: &TitleId, digest: &str) -> Result<(), Self::Error>;
    /// Encode replace: updates `current_b3` only.
    async fn record_replace(&self, title_id: &TitleId, current_b3: &str)
    -> Result<(), Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn entry_rejects_non_blake3_hex() {
        let title = TitleId::movie("603").expect("title");
        assert_eq!(
            TitleIndexEntry::new(title.clone(), "abc", A).expect_err("short"),
            TitleIndexError::InvalidDigest
        );
        let upper = A.to_ascii_uppercase();
        assert_eq!(
            TitleIndexEntry::new(title.clone(), &upper, A).expect_err("upper"),
            TitleIndexError::InvalidDigest
        );
        let entry = TitleIndexEntry::new(title.clone(), A, B).expect("ok");
        assert_eq!(entry.title_id(), &title);
        assert_eq!(entry.install_b3(), A);
        assert_eq!(entry.current_b3(), B);
    }
}
