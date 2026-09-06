//! Shared home-role paths. Resolving a path never creates a file or directory.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Active config directory, kept out of a dotfiles worktree by default.
pub fn default_config_dir() -> PathBuf {
    config_dir(&environment)
}

pub fn default_tls_dir() -> PathBuf {
    default_config_dir().join("tls")
}

/// Application state directory, including the `mediaops` component.
pub fn default_state_dir() -> PathBuf {
    state_dir(&environment)
}

pub fn default_api_socket() -> PathBuf {
    socket_path(&environment, "mediaops-api.sock")
}

pub fn default_gateway_socket() -> PathBuf {
    socket_path(&environment, "mediaopsd.sock")
}

fn environment(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

fn home(get: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    get("HOME").filter(|p| !p.is_empty()).map(PathBuf::from)
}

fn xdg(get: &impl Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    get(name).map(PathBuf::from).filter(|p| p.is_absolute())
}

fn config_dir(get: &impl Fn(&str) -> Option<OsString>) -> PathBuf {
    if let Some(dir) = get("MEDIAOPS_CONFIG_DIR").filter(|p| !p.is_empty()) {
        return PathBuf::from(dir);
    }
    let Some(config) = xdg(get, "XDG_CONFIG_HOME").or_else(|| home(get).map(|h| h.join(".config")))
    else {
        return PathBuf::from(".mediaops-config");
    };
    let candidate = config.join("mediaops");
    if !in_git_worktree(&candidate) {
        return candidate;
    }
    xdg(get, "XDG_DATA_HOME")
        .or_else(|| home(get).map(|h| h.join(".local/share")))
        .map(|p| p.join("mediaops"))
        .unwrap_or_else(|| PathBuf::from(".mediaops-data"))
}

fn state_dir(get: &impl Fn(&str) -> Option<OsString>) -> PathBuf {
    xdg(get, "XDG_STATE_HOME")
        .or_else(|| home(get).map(|h| h.join(".local/state")))
        .map(|p| p.join("mediaops"))
        .unwrap_or_else(|| PathBuf::from(".mediaops-state"))
}

fn socket_path(get: &impl Fn(&str) -> Option<OsString>, name: &str) -> PathBuf {
    xdg(get, "XDG_RUNTIME_DIR")
        .unwrap_or_else(|| state_dir(get))
        .join(name)
}

fn in_git_worktree(path: &Path) -> bool {
    // Check both lexical ancestors and each existing ancestor's real path:
    // ~/.config can be a symlink into a checkout, and mediaops may not exist.
    path.ancestors().any(|ancestor| {
        has_git_marker(ancestor)
            || match ancestor.canonicalize() {
                Ok(real) => real.ancestors().any(has_git_marker),
                Err(err) => err.kind() != std::io::ErrorKind::NotFound,
            }
    })
}

fn has_git_marker(path: &Path) -> bool {
    match std::fs::symlink_metadata(path.join(".git")) {
        Ok(_) => true, // Ordinary checkout, linked worktree, or a symlink.
        Err(err) => err.kind() != std::io::ErrorKind::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "mediaops-paths-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            std::fs::create_dir(&path).expect("scratch");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn xdg_and_explicit_overrides_have_consistent_precedence() {
        let dir = Scratch::new();
        let get = |name: &str| match name {
            "HOME" => Some(dir.0.clone().into_os_string()),
            "XDG_CONFIG_HOME" => Some(dir.0.join("configuration").into_os_string()),
            "XDG_STATE_HOME" => Some(dir.0.join("state").into_os_string()),
            "XDG_RUNTIME_DIR" => Some(dir.0.join("runtime").into_os_string()),
            _ => None,
        };
        assert_eq!(config_dir(&get), dir.0.join("configuration/mediaops"));
        assert_eq!(state_dir(&get), dir.0.join("state/mediaops"));
        assert_eq!(
            socket_path(&get, "api.sock"),
            dir.0.join("runtime/api.sock")
        );
        assert_eq!(
            config_dir(&|name| if name == "MEDIAOPS_CONFIG_DIR" {
                Some(dir.0.join("override").into_os_string())
            } else {
                get(name)
            }),
            dir.0.join("override")
        );
    }

    #[test]
    fn empty_or_relative_xdg_values_do_not_make_roles_diverge() {
        let dir = Scratch::new();
        let get = |name: &str| match name {
            "HOME" => Some(dir.0.clone().into_os_string()),
            "XDG_RUNTIME_DIR" | "MEDIAOPS_CONFIG_DIR" => Some(OsString::new()),
            "XDG_CONFIG_HOME" | "XDG_STATE_HOME" => Some("relative".into()),
            _ => None,
        };
        assert_eq!(config_dir(&get), dir.0.join(".config/mediaops"));
        assert_eq!(
            socket_path(&get, "gw.sock"),
            dir.0.join(".local/state/mediaops/gw.sock")
        );
    }

    #[test]
    fn git_directory_or_worktree_file_selects_the_data_directory() {
        let dir = Scratch::new();
        let config = dir.0.join(".config");
        std::fs::create_dir(&config).expect("config");
        std::fs::write(config.join(".git"), "gitdir: elsewhere").expect("worktree marker");
        let get = |name: &str| (name == "HOME").then(|| dir.0.clone().into_os_string());
        assert_eq!(config_dir(&get), dir.0.join(".local/share/mediaops"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_dotfiles_checkout_is_detected_before_config_exists() {
        let dir = Scratch::new();
        let checkout = dir.0.join("checkout");
        std::fs::create_dir_all(checkout.join(".git")).expect("checkout");
        std::os::unix::fs::symlink(&checkout, dir.0.join(".config")).expect("symlink");
        let get = |name: &str| (name == "HOME").then(|| dir.0.clone().into_os_string());
        assert_eq!(config_dir(&get), dir.0.join(".local/share/mediaops"));
    }
}
