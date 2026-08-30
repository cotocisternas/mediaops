//! Range-concurrency probe types. Persistence lives in `store` behind
//! [`ProbeRepo`]; hashing is pure.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnderlayMode {
    Direct,
    Tailscale,
    #[serde(rename = "wireguard", alias = "wire_guard")]
    WireGuard,
}

impl UnderlayMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Tailscale => "tailscale",
            Self::WireGuard => "wireguard",
        }
    }

    pub fn parse(name: &str) -> Result<Self, ProbeError> {
        match name {
            "direct" => Ok(Self::Direct),
            "tailscale" => Ok(Self::Tailscale),
            "wireguard" | "wire_guard" => Ok(Self::WireGuard),
            other => Err(ProbeError::UnknownUnderlay(other.to_string())),
        }
    }
}

impl Default for UnderlayMode {
    fn default() -> Self {
        Self::Direct
    }
}

/// Hash of seedbox address + underlay mode. Keys the `probes` table (AD-12).
pub fn endpoint_fingerprint(address: &str, underlay: UnderlayMode) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(address.as_bytes());
    hasher.update(&[0]);
    hasher.update(underlay.as_str().as_bytes());
    hasher.finalize().to_hex().to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub endpoint_fingerprint: String,
    pub range_concurrency: u32,
}

/// Probes repository port (AD-8). Adapter lives in `store`.
///
/// A trait, not I/O: async signatures only. Bootstrap keeps calling the
/// inherent `Store` methods; this trait is the type the composition root injects.
#[allow(async_fn_in_trait)]
pub trait ProbeRepo: Send + Sync {
    type Error;

    async fn get_probe(&self, fingerprint: &str) -> Result<Option<Probe>, Self::Error>;
    async fn put_probe(&self, probe: &Probe) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProbeError {
    #[error("unknown underlay `{0}`")]
    UnknownUnderlay(String),
    #[error("no throughput samples")]
    EmptySamples,
}

/// Raise N until throughput plateaus (next step improves by ≤ 5%).
pub fn plateau_n(samples: &[(u32, u64)]) -> Result<u32, ProbeError> {
    let Some((first_n, mut best_t)) = samples.first().copied() else {
        return Err(ProbeError::EmptySamples);
    };
    let mut best_n = first_n;
    for &(n, t) in &samples[1..] {
        if t > best_t && (t - best_t).saturating_mul(20) > best_t {
            best_n = n;
            best_t = t;
        } else {
            break;
        }
    }
    Ok(best_n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_changes_with_address_or_underlay() {
        let a = endpoint_fingerprint("seedbox.example:50051", UnderlayMode::Direct);
        let b = endpoint_fingerprint("other.example:50051", UnderlayMode::Direct);
        let c = endpoint_fingerprint("seedbox.example:50051", UnderlayMode::Tailscale);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(
            a,
            endpoint_fingerprint("seedbox.example:50051", UnderlayMode::Direct)
        );
    }

    #[test]
    fn plateau_picks_last_meaningful_gain() {
        let n = plateau_n(&[(1, 10), (2, 20), (3, 21), (4, 21)]).expect("samples");
        assert_eq!(n, 2);
    }

    #[test]
    fn wireguard_toml_tag_is_wireguard() {
        #[derive(serde::Deserialize, serde::Serialize)]
        struct Wrap {
            underlay: UnderlayMode,
        }
        let parsed: Wrap = toml::from_str("underlay = \"wireguard\"").expect("deserialize");
        assert_eq!(parsed.underlay, UnderlayMode::WireGuard);
        assert_eq!(parsed.underlay.as_str(), "wireguard");
        assert_eq!(
            UnderlayMode::parse("wire_guard").expect("alias"),
            UnderlayMode::WireGuard
        );
        let encoded = toml::to_string(&parsed).expect("serialize");
        assert!(encoded.contains("wireguard"), "{encoded}");
        assert!(!encoded.contains("wire_guard"), "{encoded}");
        let fp = endpoint_fingerprint("seedbox.example:50051", UnderlayMode::WireGuard);
        assert_eq!(
            fp,
            endpoint_fingerprint(
                "seedbox.example:50051",
                UnderlayMode::parse("wireguard").expect("parse")
            )
        );
        assert_eq!(fp.len(), 64);
    }
}
