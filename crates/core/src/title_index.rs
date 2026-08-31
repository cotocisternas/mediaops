//! Dual-digest title index (AD-8). Pure types and the repository port.
//!
//! `install_b3` is the reclaim/local-proof digest, written once by
//! [`TitleIndexRepo::record_install`] after a successful [`crate::install::install`].
//! `current_b3` is what `verify` checks: that same call sets it, and only
//! [`TitleIndexRepo::record_replace`] (after encode's [`crate::install::replace`])
//! updates it afterwards.

use crate::digest::Blake3Hex;
use crate::title_id::TitleId;

/// One `title_index` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleIndexEntry {
    title_id: TitleId,
    install_b3: Blake3Hex,
    current_b3: Blake3Hex,
}

impl TitleIndexEntry {
    pub fn new(title_id: TitleId, install_b3: Blake3Hex, current_b3: Blake3Hex) -> Self {
        Self {
            title_id,
            install_b3,
            current_b3,
        }
    }

    pub fn title_id(&self) -> &TitleId {
        &self.title_id
    }

    pub fn install_b3(&self) -> &Blake3Hex {
        &self.install_b3
    }

    pub fn current_b3(&self) -> &Blake3Hex {
        &self.current_b3
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TitleIndexError {
    #[error("install_b3 is immutable")]
    InstallDigestImmutable,
    #[error("no title_index row to replace")]
    NotInstalled,
}

/// Persistence door for the install gate. Adapter lives in `store`.
///
/// A trait, not I/O: async signatures only. The filesystem gate does not
/// call this; the composition root does, after `install` / `replace`.
#[allow(async_fn_in_trait)]
pub trait TitleIndexRepo: Send + Sync {
    type Error;

    async fn get(&self, title_id: &TitleId) -> Result<Option<TitleIndexEntry>, Self::Error>;
    /// First placement: writes `install_b3` and `current_b3`. `install_b3` never changes.
    async fn record_install(
        &self,
        title_id: &TitleId,
        digest: &Blake3Hex,
    ) -> Result<(), Self::Error>;
    /// Encode replace: updates `current_b3` only.
    async fn record_replace(
        &self,
        title_id: &TitleId,
        current_b3: &Blake3Hex,
    ) -> Result<(), Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(fill: char) -> Blake3Hex {
        Blake3Hex::parse(&fill.to_string().repeat(64)).expect("digest")
    }

    #[test]
    fn entry_stores_distinct_install_and_current() {
        let title = TitleId::movie("603").expect("title");
        let install = digest('a');
        let current = digest('b');
        let entry = TitleIndexEntry::new(title.clone(), install.clone(), current.clone());
        assert_eq!(entry.title_id(), &title);
        assert_eq!(entry.install_b3(), &install);
        assert_eq!(entry.current_b3(), &current);
    }
}
