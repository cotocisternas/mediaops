//! Unified diffs for ini/xml/nginx. One `similar` module (AD-15 convention).

use similar::TextDiff;

/// Unified diff of `old` → `new` labeled with `path`. Empty when equal.
pub fn unified_diff(old: &str, new: &str, path: &str) -> String {
    if old == new {
        return String::new();
    }
    TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

/// blake3 of name-sorted nginx app confs (panel fingerprint).
pub fn panel_fingerprint(files: &[(String, &[u8])]) -> String {
    let mut items: Vec<&(String, &[u8])> = files.iter().collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = blake3::Hasher::new();
    for (name, bytes) in items {
        hasher.update(name.as_bytes());
        hasher.update(&[0]);
        hasher.update(bytes);
    }
    hasher.finalize().to_hex().to_string()
}

/// EdgeInvariant nginx Host header: `$host`, not a rewritten localhost.
pub fn nginx_host_ok(conf: &str) -> bool {
    conf.contains("Host $host") || conf.contains("host $host")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_diff_empty_when_equal() {
        assert_eq!(unified_diff("a\n", "a\n", "x.conf"), "");
    }

    #[test]
    fn unified_diff_renders_path_and_change() {
        let diff = unified_diff("bind=*\n", "bind=127.0.0.1\n", "config.xml");
        assert!(diff.contains("a/config.xml"));
        assert!(diff.contains("b/config.xml"));
        assert!(diff.contains("-bind=*"));
        assert!(diff.contains("+bind=127.0.0.1"));
    }

    #[test]
    fn panel_host_rewrite_changes_fingerprint() {
        let good = b"proxy_set_header Host $host;\n";
        let rewritten = b"proxy_set_header Host 127.0.0.1;\n";
        let a = panel_fingerprint(&[("sonarr.conf".into(), good.as_slice())]);
        let b = panel_fingerprint(&[("sonarr.conf".into(), rewritten.as_slice())]);
        assert_ne!(a, b);
        assert!(nginx_host_ok(std::str::from_utf8(good).expect("utf8")));
        assert!(!nginx_host_ok(
            std::str::from_utf8(rewritten).expect("utf8")
        ));
    }
}
