//! Discover API keys from on-box `config.xml` / `sabnzbd.ini`. Never echo them.

use std::fmt;
use std::path::{Path, PathBuf};

use mediaops_core::KeyPresence;

const MASKED: &str = "********";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("masked API key refused")]
    MaskedKey,
    #[error("key discovery: {0}")]
    Io(String),
}

/// Paths of grabber config files on the seedbox. Tests inject a tempdir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPaths {
    pub sonarr: PathBuf,
    pub radarr: PathBuf,
    pub lidarr: PathBuf,
    pub prowlarr: PathBuf,
    pub sab: PathBuf,
    pub qbit: PathBuf,
}

impl KeyPaths {
    pub fn from_home(home: &Path) -> Self {
        Self {
            sonarr: home.join(".config/Sonarr/config.xml"),
            radarr: home.join(".config/Radarr/config.xml"),
            lidarr: home.join(".config/Lidarr/config.xml"),
            prowlarr: home.join(".config/Prowlarr/config.xml"),
            sab: home.join(".config/sabnzbd/sabnzbd.ini"),
            qbit: home.join(".config/qBittorrent/qBittorrent.conf"),
        }
    }
}

/// Keys stay in this type. Debug/Display never print material.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct DiscoveredKeys {
    sonarr: Option<String>,
    radarr: Option<String>,
    lidarr: Option<String>,
    prowlarr: Option<String>,
    sab: Option<String>,
    qbit: Option<String>,
}

impl fmt::Debug for DiscoveredKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiscoveredKeys")
            .field("sonarr", &self.sonarr.as_ref().map(|_| true))
            .field("radarr", &self.radarr.as_ref().map(|_| true))
            .field("lidarr", &self.lidarr.as_ref().map(|_| true))
            .field("prowlarr", &self.prowlarr.as_ref().map(|_| true))
            .field("sab", &self.sab.as_ref().map(|_| true))
            .field("qbit", &self.qbit.as_ref().map(|_| true))
            .finish()
    }
}

impl DiscoveredKeys {
    pub fn presence(&self) -> KeyPresence {
        KeyPresence {
            sonarr_key_present: self.sonarr.is_some(),
            radarr_key_present: self.radarr.is_some(),
            lidarr_key_present: self.lidarr.is_some(),
            prowlarr_key_present: self.prowlarr.is_some(),
            sab_key_present: self.sab.is_some(),
            qbit_key_present: self.qbit.is_some(),
        }
    }

    pub fn sonarr(&self) -> Option<&str> {
        self.sonarr.as_deref()
    }
    pub fn radarr(&self) -> Option<&str> {
        self.radarr.as_deref()
    }
    pub fn lidarr(&self) -> Option<&str> {
        self.lidarr.as_deref()
    }
    pub fn prowlarr(&self) -> Option<&str> {
        self.prowlarr.as_deref()
    }
    pub fn sab(&self) -> Option<&str> {
        self.sab.as_deref()
    }
    pub fn qbit(&self) -> Option<&str> {
        self.qbit.as_deref()
    }
}

pub fn is_masked_key(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed == MASKED || (!trimmed.is_empty() && trimmed.chars().all(|c| c == '*'))
}

pub fn refuse_masked(value: &str) -> Result<(), KeyError> {
    if is_masked_key(value) {
        Err(KeyError::MaskedKey)
    } else {
        Ok(())
    }
}

pub fn discover_servarr_key(config_xml: &str) -> Result<Option<String>, KeyError> {
    let Some(key) = xml_tag(config_xml, "ApiKey") else {
        return Ok(None);
    };
    if key.is_empty() {
        return Ok(None);
    }
    refuse_masked(&key)?;
    Ok(Some(key))
}

pub fn discover_sab_key(ini: &str) -> Result<Option<String>, KeyError> {
    let Some(key) = ini_value(ini, "api_key") else {
        return Ok(None);
    };
    if key.is_empty() {
        return Ok(None);
    }
    refuse_masked(&key)?;
    Ok(Some(key))
}

pub fn discover_keys(paths: &KeyPaths) -> Result<DiscoveredKeys, KeyError> {
    let sab = match std::fs::read_to_string(&paths.sab) {
        Ok(text) => discover_sab_key(&text)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(KeyError::Io(err.to_string())),
    };
    Ok(DiscoveredKeys {
        sonarr: read_servarr(&paths.sonarr)?,
        radarr: read_servarr(&paths.radarr)?,
        lidarr: read_servarr(&paths.lidarr)?,
        prowlarr: read_servarr(&paths.prowlarr)?,
        sab,
        qbit: if paths.qbit.is_file() {
            Some("present".into())
        } else {
            None
        },
    })
}

fn read_servarr(path: &Path) -> Result<Option<String>, KeyError> {
    match std::fs::read_to_string(path) {
        Ok(text) => discover_servarr_key(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(KeyError::Io(err.to_string())),
    }
}

fn xml_tag(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].trim().to_string())
}

fn ini_value(body: &str, key: &str) -> Option<String> {
    for line in body.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(value) = rest.strip_prefix('=') else {
            continue;
        };
        return Some(value.trim().to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masked_stars_are_refused() {
        assert!(is_masked_key("********"));
        assert!(is_masked_key("****"));
        assert_eq!(
            discover_servarr_key("<Config><ApiKey>********</ApiKey></Config>"),
            Err(KeyError::MaskedKey)
        );
        assert_eq!(
            discover_sab_key("[misc]\napi_key = ********\n"),
            Err(KeyError::MaskedKey)
        );
        assert!(refuse_masked("real-key-not-stars").is_ok());
    }

    #[test]
    fn servarr_and_sab_keys_parse_without_echo() {
        let xml = "<Config>\n  <ApiKey>abcdef0123456789abcdef0123456789</ApiKey>\n</Config>";
        let key = discover_servarr_key(xml).expect("xml").expect("key");
        assert_eq!(key, "abcdef0123456789abcdef0123456789");
        let ini = "[misc]\napi_key = sab-secret-value\n";
        let sab = discover_sab_key(ini).expect("ini").expect("key");
        assert_eq!(sab, "sab-secret-value");
        let discovered = DiscoveredKeys {
            sonarr: Some(key),
            sab: Some(sab),
            ..DiscoveredKeys::default()
        };
        let debug = format!("{discovered:?}");
        assert!(!debug.contains("abcdef"));
        assert!(!debug.contains("sab-secret"));
        assert!(debug.contains("sonarr: Some(true)"));
        let presence = discovered.presence();
        assert!(presence.sonarr_key_present);
        assert!(presence.sab_key_present);
        assert!(!presence.radarr_key_present);
    }

    #[test]
    fn missing_files_are_absence_not_error() {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-arr-keys-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let paths = KeyPaths::from_home(&dir);
        let keys = discover_keys(&paths).expect("discover");
        assert_eq!(keys.presence(), KeyPresence::default());

        for (rel, body) in [
            (
                ".config/Sonarr/config.xml",
                "<Config><ApiKey>sonarr-key</ApiKey></Config>",
            ),
            (
                ".config/Radarr/config.xml",
                "<Config><ApiKey>radarr-key</ApiKey></Config>",
            ),
            (
                ".config/Lidarr/config.xml",
                "<Config><ApiKey>lidarr-key</ApiKey></Config>",
            ),
            (
                ".config/Prowlarr/config.xml",
                "<Config><ApiKey>prowlarr-key</ApiKey></Config>",
            ),
            (".config/sabnzbd/sabnzbd.ini", "[misc]\napi_key = sab-key\n"),
            (".config/qBittorrent/qBittorrent.conf", "[Preferences]\n"),
        ] {
            let path = dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("mkdir");
            }
            std::fs::write(&path, body).expect("write");
        }
        let keys = discover_keys(&paths).expect("discover");
        let presence = keys.presence();
        assert!(presence.sonarr_key_present);
        assert!(presence.radarr_key_present);
        assert!(presence.lidarr_key_present);
        assert!(presence.prowlarr_key_present);
        assert!(presence.sab_key_present);
        assert!(presence.qbit_key_present);
        assert_eq!(keys.sonarr(), Some("sonarr-key"));
        assert_eq!(keys.radarr(), Some("radarr-key"));
        assert_eq!(keys.lidarr(), Some("lidarr-key"));
        assert_eq!(keys.prowlarr(), Some("prowlarr-key"));
        assert_eq!(keys.sab(), Some("sab-key"));
        assert!(keys.qbit().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_api_key_tag_is_absence() {
        assert_eq!(
            discover_servarr_key("<Config><ApiKey></ApiKey></Config>").expect("empty"),
            None
        );
        assert_eq!(discover_sab_key("api_key =\n").expect("empty"), None);
        assert_eq!(
            discover_servarr_key("<Config></Config>").expect("none"),
            None
        );
    }
}
