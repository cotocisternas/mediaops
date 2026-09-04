//! Discover API keys from on-box `config.xml` / `sabnzbd.ini`. Never echo them.

use std::fmt;
use std::path::{Path, PathBuf};

use mediaops_core::KeyPresence;

const MASKED: &str = "********";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("masked API key refused")]
    MaskedKey,
    #[error("empty API key refused")]
    EmptyKey,
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
    qbit_present: bool,
}

impl fmt::Debug for DiscoveredKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiscoveredKeys")
            .field("sonarr", &self.sonarr.as_ref().map(|_| true))
            .field("radarr", &self.radarr.as_ref().map(|_| true))
            .field("lidarr", &self.lidarr.as_ref().map(|_| true))
            .field("prowlarr", &self.prowlarr.as_ref().map(|_| true))
            .field("sab", &self.sab.as_ref().map(|_| true))
            .field("qbit", &self.qbit_present)
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
            qbit_key_present: self.qbit_present,
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
    pub fn qbit_present(&self) -> bool {
        self.qbit_present
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

/// Refuse masked stars and blank keys. Discovery treats empty tags as absence; clients use this.
pub fn refuse_key(value: &str) -> Result<(), KeyError> {
    if value.trim().is_empty() {
        Err(KeyError::EmptyKey)
    } else {
        refuse_masked(value)
    }
}

pub fn discover_servarr_key(config_xml: &str) -> Result<Option<String>, KeyError> {
    let Some(key) = xml_tag(config_xml, "ApiKey")? else {
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
        qbit_present: qbit_config_present(&paths.qbit)?,
    })
}

/// Where each app listens, read from the same config files as the keys.
///
/// Swizzin boxes put SABnzbd and qBittorrent on per-user ports (this one:
/// 65080 and 9148), so a hard-coded 8080 would talk to nothing. Missing files
/// or fields fall back to the stock defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredEndpoints {
    pub sonarr: Endpoint,
    pub radarr: Endpoint,
    pub lidarr: Endpoint,
    pub prowlarr: Endpoint,
    pub sab: Endpoint,
    pub qbit: Endpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub port: u16,
    /// Leading-slash URL base, or empty for none.
    pub url_base: String,
}

impl Endpoint {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}{}", self.port, self.url_base)
    }
}

fn normalize_url_base(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("/{trimmed}")
    }
}

fn servarr_endpoint(
    path: &Path,
    default_port: u16,
    default_base: &str,
) -> Result<Endpoint, KeyError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Endpoint {
                port: default_port,
                url_base: normalize_url_base(default_base),
            });
        }
        Err(err) => return Err(KeyError::Io(err.to_string())),
    };
    let port = xml_tag(&text, "Port")?
        .and_then(|p| p.parse().ok())
        .unwrap_or(default_port);
    let url_base = xml_tag(&text, "UrlBase")?
        .map(|b| normalize_url_base(&b))
        .unwrap_or_else(|| normalize_url_base(default_base));
    Ok(Endpoint { port, url_base })
}

fn sab_endpoint(path: &Path) -> Result<Endpoint, KeyError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Endpoint {
                port: 8080,
                url_base: "/sabnzbd".into(),
            });
        }
        Err(err) => return Err(KeyError::Io(err.to_string())),
    };
    let port = ini_value(&text, "port")
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let url_base = ini_value(&text, "url_base")
        .map(|b| normalize_url_base(&b))
        .unwrap_or_else(|| "/sabnzbd".into());
    Ok(Endpoint { port, url_base })
}

fn qbit_endpoint(path: &Path) -> Result<Endpoint, KeyError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Endpoint {
                port: 8080,
                url_base: String::new(),
            });
        }
        Err(err) => return Err(KeyError::Io(err.to_string())),
    };
    let port = text
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("WebUI\\Port="))
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(8080);
    Ok(Endpoint {
        port,
        url_base: String::new(),
    })
}

pub fn discover_endpoints(paths: &KeyPaths) -> Result<DiscoveredEndpoints, KeyError> {
    Ok(DiscoveredEndpoints {
        sonarr: servarr_endpoint(&paths.sonarr, 8989, "/sonarr")?,
        radarr: servarr_endpoint(&paths.radarr, 7878, "/radarr")?,
        lidarr: servarr_endpoint(&paths.lidarr, 8686, "/lidarr")?,
        prowlarr: servarr_endpoint(&paths.prowlarr, 9696, "/prowlarr")?,
        sab: sab_endpoint(&paths.sab)?,
        qbit: qbit_endpoint(&paths.qbit)?,
    })
}

fn qbit_config_present(path: &Path) -> Result<bool, KeyError> {
    match std::fs::metadata(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(KeyError::Io(err.to_string())),
        Ok(meta) => Ok(meta.is_file()),
    }
}

fn read_servarr(path: &Path) -> Result<Option<String>, KeyError> {
    match std::fs::read_to_string(path) {
        Ok(text) => discover_servarr_key(&text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(KeyError::Io(err.to_string())),
    }
}

fn xml_tag(body: &str, tag: &str) -> Result<Option<String>, KeyError> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(start_at) = body.find(&open) else {
        return Ok(None);
    };
    let start = start_at + open.len();
    let Some(rel) = body[start..].find(&close) else {
        return Err(KeyError::Io(format!("malformed <{tag}>")));
    };
    Ok(Some(body[start..start + rel].trim().to_string()))
}

fn ini_value(body: &str, key: &str) -> Option<String> {
    let mut in_misc = false;
    let mut saw_section = false;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            saw_section = true;
            in_misc = line.eq_ignore_ascii_case("[misc]");
            continue;
        }
        if saw_section && !in_misc {
            continue;
        }
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(value) = rest.strip_prefix('=') else {
            continue;
        };
        let value = value
            .split_once([';', '#'])
            .map(|(v, _)| v)
            .unwrap_or(value)
            .trim();
        return Some(value.to_string());
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
        assert_eq!(refuse_key(""), Err(KeyError::EmptyKey));
        assert_eq!(refuse_key("   "), Err(KeyError::EmptyKey));
    }

    #[test]
    fn endpoints_come_from_the_config_files_not_defaults() {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-endpoints-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        let paths = KeyPaths::from_home(&dir);
        for (path, body) in [
            (
                &paths.sonarr,
                "<Config><Port>8989</Port><UrlBase>sonarr</UrlBase></Config>",
            ),
            (
                &paths.radarr,
                "<Config><Port>7878</Port><UrlBase>/radarr/</UrlBase></Config>",
            ),
            (
                &paths.sab,
                "[misc]\nhost = 127.0.0.1\nport = 65080\nurl_base = /sabnzbd\n[servers]\nport = 563\n",
            ),
            (
                &paths.qbit,
                "[Preferences]\nWebUI\\Address=*\nWebUI\\LocalHostAuth=false\nWebUI\\Port=9148\n",
            ),
        ] {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, body).expect("write");
        }
        let endpoints = discover_endpoints(&paths).expect("endpoints");
        assert_eq!(endpoints.sonarr.base_url(), "http://127.0.0.1:8989/sonarr");
        assert_eq!(endpoints.radarr.base_url(), "http://127.0.0.1:7878/radarr");
        assert_eq!(endpoints.sab.base_url(), "http://127.0.0.1:65080/sabnzbd");
        assert_eq!(endpoints.qbit.base_url(), "http://127.0.0.1:9148");
        // Missing files fall back to stock defaults.
        assert_eq!(endpoints.lidarr.base_url(), "http://127.0.0.1:8686/lidarr");
        assert_eq!(
            endpoints.prowlarr.base_url(),
            "http://127.0.0.1:9696/prowlarr"
        );
        let _ = std::fs::remove_dir_all(dir);
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
        assert!(keys.qbit_present());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_api_key_tag_is_absence() {
        assert_eq!(
            discover_servarr_key("<Config><ApiKey></ApiKey></Config>").expect("empty"),
            None
        );
        assert_eq!(
            discover_sab_key("[misc]\napi_key =\n").expect("empty"),
            None
        );
        assert_eq!(
            discover_servarr_key("<Config></Config>").expect("none"),
            None
        );
    }

    #[test]
    fn unclosed_apikey_is_malformed_not_absence() {
        let err = discover_servarr_key("<Config><ApiKey>abc").expect_err("malformed");
        assert!(matches!(err, KeyError::Io(_)));
    }

    #[test]
    fn sab_key_comes_from_misc_not_other_sections() {
        let ini = "[servers]\napi_key = decoy\n[misc]\napi_key = sab-secret-value ; comment\n";
        let sab = discover_sab_key(ini).expect("ini").expect("key");
        assert_eq!(sab, "sab-secret-value");
    }
}
