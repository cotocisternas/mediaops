//! AD-2 workspace dependency law. Walks Cargo edges, not mermaid provider arrows.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use cargo_metadata::Metadata;

/// Allowed depender → dependee workspace Cargo edges (inverted spine mermaid).
const ALLOWED_WORKSPACE_EDGES: &[(&str, &str)] = &[
    ("mediaops-proto", "mediaops-core"),
    ("mediaops-store", "mediaops-core"),
    ("mediaops-net", "mediaops-core"),
    ("mediaops-ssh", "mediaops-core"),
    ("mediaops-arr", "mediaops-core"),
    ("mediaops-transfer", "mediaops-core"),
    ("mediaops-sync", "mediaops-core"),
    ("mediaops-encode", "mediaops-core"),
    ("mediaops", "mediaops-core"),
    ("mediaopsd", "mediaops-core"),
    ("mediaops-net", "mediaops-proto"),
    ("mediaops-transfer", "mediaops-proto"),
    ("mediaops", "mediaops-proto"),
    ("mediaopsd", "mediaops-proto"),
    ("mediaops-transfer", "mediaops-net"),
    ("mediaopsd", "mediaops-net"),
    ("mediaopsd", "mediaops-arr"),
    ("mediaops-sync", "mediaops-transfer"),
    ("mediaops", "mediaops-transfer"),
    ("mediaops", "mediaops-store"),
    ("mediaops", "mediaops-ssh"),
    ("mediaops", "mediaops-sync"),
    ("mediaops", "mediaops-encode"),
];

const BANNED_DIRECT_CRATES: &[&str] = &[
    "rsync",
    "rclone",
    "ftp",
    "ssh2",
    "russh",
    "ffmpeg-next",
    "native-tls",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub message: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

pub fn violations(metadata: &Metadata) -> Vec<Violation> {
    let allowed: HashSet<(&str, &str)> = ALLOWED_WORKSPACE_EDGES.iter().copied().collect();
    let members = metadata.workspace_packages();
    let member_names: HashSet<String> = members.iter().map(|p| p.name.to_string()).collect();

    let mut found = Vec::new();
    let mut workspace_graph: HashMap<String, Vec<String>> = HashMap::new();

    for package in &members {
        let from = package.name.to_string();
        let mut tos = Vec::new();
        for dep in &package.dependencies {
            let to = dep.name.as_str();
            if member_names.contains(to) {
                tos.push(to.to_string());
                if !allowed.contains(&(from.as_str(), to)) {
                    found.push(Violation {
                        message: format!("illegal workspace edge: {from} → {to}"),
                    });
                }
            }
            if to == "reqwest" && from != "mediaops-arr" {
                found.push(Violation {
                    message: format!(
                        "reqwest is a direct dependency of {from}; only mediaops-arr may depend on reqwest"
                    ),
                });
            }
            if to == "rusqlite" && from != "mediaops-store" {
                found.push(Violation {
                    message: format!(
                        "rusqlite is a direct dependency of {from}; only mediaops-store may depend on rusqlite"
                    ),
                });
            }
            if BANNED_DIRECT_CRATES.contains(&to) {
                found.push(Violation {
                    message: format!("banned crate {to} is a direct dependency of {from}"),
                });
            }
        }
        workspace_graph.insert(from, tos);
    }

    for forbidden in ["mediaops-store", "mediaops-encode"] {
        if closure_contains(&workspace_graph, "mediaopsd", forbidden) {
            found.push(Violation {
                message: format!(
                    "{forbidden} is in mediaopsd's workspace-internal transitive closure"
                ),
            });
        }
    }

    found
}

/// Whether the text right after a `#[cfg(test)]` marker opens the test module.
fn opens_test_module(after_marker: &str) -> bool {
    let head = after_marker.trim_start();
    head.starts_with("mod tests {") || head.starts_with("pub(crate) mod tests {")
}

fn closure_contains(graph: &HashMap<String, Vec<String>>, start: &str, needle: &str) -> bool {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(start.to_string());
    while let Some(node) = queue.pop_front() {
        if !seen.insert(node.clone()) {
            continue;
        }
        let Some(edges) = graph.get(&node) else {
            continue;
        };
        for next in edges {
            if next == needle {
                return true;
            }
            queue.push_back(next.clone());
        }
    }
    false
}

/// Modules in `mediaops-core` allowed to touch the filesystem.
///
/// Story 1.2 carved `walker` and `install` out of the "core is pure domain (no
/// I/O)" law so they could use caller-supplied roots. That carve-out lived only
/// in a doc comment, which meant any future `core` module could quietly widen
/// it. This keeps it enforced, the same way AD-2 is.
pub const CORE_IO_MODULES: &[&str] = &["walker.rs", "install.rs"];

/// Paths under `crates/core/src` that reference `std::fs` outside the carve-out.
pub fn core_io_violations(core_src: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(core_src) else {
        return vec![format!("cannot read {}", core_src.display())];
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        if CORE_IO_MODULES.contains(&name.as_str()) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Only the code, not the tests that build tempdir fixtures. The scan
        // stops at the first `#[cfg(test)]`, so a test-only item in the middle
        // of a file would hide every line below it from this check.
        if let Some((_, after_marker)) = source.split_once("\n#[cfg(test)]")
            && !opens_test_module(after_marker)
        {
            found.push(format!(
                "{name} has a #[cfg(test)] item before `mod tests`, which truncates this scan; \
                 move test-only items inside the test module"
            ));
        }
        let code = source
            .split("\n#[cfg(test)]")
            .next()
            .unwrap_or(&source)
            .to_string();
        let hits: Vec<&str> = ["std::fs", "fs::read", "fs::write", "fs::rename", "fs::File"]
            .into_iter()
            .filter(|needle| code.contains(needle))
            .collect();
        if !hits.is_empty() {
            found.push(format!("{name} references {hits:?}"));
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use cargo_metadata::MetadataCommand;

    fn live_metadata() -> Metadata {
        MetadataCommand::new()
            .manifest_path(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
            .no_deps()
            .other_options(["--offline".to_string()])
            .exec()
            .expect("cargo metadata from workspace root")
    }

    fn add_direct_dep(metadata: &mut Metadata, package_name: &str, dep_name: &str) {
        let template = metadata
            .packages
            .iter()
            .find(|p| p.name.as_ref() == "mediaops-core")
            .and_then(|p| p.dependencies.first())
            .cloned()
            .expect("mediaops-core has a dependency to clone as a fixture template");
        let package = metadata
            .packages
            .iter_mut()
            .find(|p| p.name.as_ref() == package_name)
            .unwrap_or_else(|| panic!("workspace package {package_name}"));
        let mut dep = template;
        dep.name = dep_name.to_string();
        package.dependencies.push(dep);
    }

    fn messages(found: &[Violation]) -> String {
        found
            .iter()
            .map(|v| v.message.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    const SEED_PACKAGES: &[&str] = &[
        "mediaops-core",
        "mediaops-proto",
        "mediaops-store",
        "mediaops-net",
        "mediaops-ssh",
        "mediaops-arr",
        "mediaops-transfer",
        "mediaops-sync",
        "mediaops-encode",
        "mediaops-arch-tests",
        "mediaops",
        "mediaopsd",
    ];

    #[test]
    fn live_workspace_is_ad2_subgraph() {
        let metadata = live_metadata();
        let names: HashSet<String> = metadata
            .workspace_packages()
            .iter()
            .map(|p| p.name.to_string())
            .collect();
        for expected in SEED_PACKAGES {
            assert!(
                names.contains(*expected),
                "workspace members must include seed package {expected}, got {names:?}"
            );
        }
        let found = violations(&metadata);
        assert!(
            found.is_empty(),
            "live workspace must be an AD-2 subgraph, got:\n{}",
            messages(&found)
        );
    }

    #[test]
    fn reqwest_outside_arr_is_violation() {
        let mut metadata = live_metadata();
        add_direct_dep(&mut metadata, "mediaops-core", "reqwest");
        let found = violations(&metadata);
        assert!(
            found
                .iter()
                .any(|v| v.message.contains("reqwest") && v.message.contains("mediaops-core")),
            "expected reqwest-on-core violation, got:\n{}",
            messages(&found)
        );
    }

    #[test]
    fn rusqlite_outside_store_is_violation() {
        let mut metadata = live_metadata();
        add_direct_dep(&mut metadata, "mediaops-core", "rusqlite");
        let found = violations(&metadata);
        assert!(
            found
                .iter()
                .any(|v| v.message.contains("rusqlite") && v.message.contains("mediaops-core")),
            "expected rusqlite-on-core violation, got:\n{}",
            messages(&found)
        );
    }

    #[test]
    fn mediaopsd_to_store_is_violation() {
        let mut metadata = live_metadata();
        add_direct_dep(&mut metadata, "mediaopsd", "mediaops-store");
        let found = violations(&metadata);
        assert!(
            found.iter().any(|v| {
                v.message.contains("workspace-internal transitive closure")
                    && v.message.contains("mediaops-store")
                    && v.message.contains("mediaopsd")
            }),
            "expected store-in-mediaopsd-closure violation, got:\n{}",
            messages(&found)
        );
    }

    #[test]
    fn encode_in_mediaopsd_closure_is_violation() {
        let mut metadata = live_metadata();
        add_direct_dep(&mut metadata, "mediaopsd", "mediaops-encode");
        let found = violations(&metadata);
        assert!(
            found.iter().any(|v| {
                v.message.contains("workspace-internal transitive closure")
                    && v.message.contains("mediaops-encode")
                    && v.message.contains("mediaopsd")
            }),
            "expected encode-in-mediaopsd-closure violation, got:\n{}",
            messages(&found)
        );
    }

    #[test]
    fn closure_contains_walks_intermediate_workspace_crate() {
        let mut graph = HashMap::new();
        graph.insert("mediaopsd".into(), vec!["mediaops-net".into()]);
        graph.insert("mediaops-net".into(), vec!["mediaops-encode".into()]);
        assert!(closure_contains(&graph, "mediaopsd", "mediaops-encode"));
        assert!(!closure_contains(&graph, "mediaopsd", "mediaops-store"));
    }

    #[test]
    fn encode_in_mediaopsd_transitive_closure_is_violation() {
        let mut metadata = live_metadata();
        add_direct_dep(&mut metadata, "mediaopsd", "mediaops-net");
        add_direct_dep(&mut metadata, "mediaops-net", "mediaops-encode");
        let found = violations(&metadata);
        assert!(
            found.iter().any(|v| {
                v.message.contains("workspace-internal transitive closure")
                    && v.message.contains("mediaops-encode")
                    && v.message.contains("mediaopsd")
            }),
            "expected mediaopsd closure violation independent of the net→encode edge, got:\n{}",
            messages(&found)
        );
    }

    #[test]
    fn banned_crate_name_is_violation() {
        let mut metadata = live_metadata();
        add_direct_dep(&mut metadata, "mediaops-net", "ssh2");
        let found = violations(&metadata);
        assert!(
            found
                .iter()
                .any(|v| v.message.contains("ssh2") && v.message.contains("mediaops-net")),
            "expected banned ssh2 violation, got:\n{}",
            messages(&found)
        );
    }

    #[test]
    fn core_filesystem_io_stays_in_walker_and_install() {
        let core_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates dir")
            .join("core")
            .join("src");
        assert!(core_src.is_dir(), "{} must exist", core_src.display());
        let violations = core_io_violations(&core_src);
        assert!(
            violations.is_empty(),
            "core is pure domain outside {CORE_IO_MODULES:?}; found: {violations:?}"
        );
    }

    #[test]
    fn core_io_violation_is_detected_when_a_pure_module_reaches_for_the_filesystem() {
        // The check must be able to fail, or it proves nothing.
        let tmp = scratch_dir("io");
        std::fs::write(tmp.join("walker.rs"), "use std::fs;\n").expect("write allowed");
        std::fs::write(
            tmp.join("pathschema.rs"),
            "fn f() { std::fs::read(\"x\"); }\n",
        )
        .expect("write violation");
        let violations = core_io_violations(&tmp);
        let _ = std::fs::remove_dir_all(&tmp);

        assert_eq!(violations.len(), 1, "got {violations:?}");
        assert!(violations[0].starts_with("pathschema.rs"));
    }

    #[test]
    fn a_test_item_above_mod_tests_is_reported_because_it_truncates_the_scan() {
        let tmp = scratch_dir("cfgtest");
        std::fs::write(
            tmp.join("desired_state.rs"),
            "fn f() {}\n#[cfg(test)]\nconst FIXTURE: &str = \"x\";\n\
             #[cfg(test)]\nmod tests {}\nfn g() { std::fs::read(\"x\"); }\n",
        )
        .expect("write mid-file cfg(test) item");
        std::fs::write(
            tmp.join("bytes.rs"),
            "fn f() {}\n#[cfg(test)]\npub(crate) mod tests {\n    use std::fs;\n}\n",
        )
        .expect("write conventional trailing test module");
        let violations = core_io_violations(&tmp);
        let _ = std::fs::remove_dir_all(&tmp);

        assert_eq!(violations.len(), 1, "got {violations:?}");
        assert!(
            violations[0].starts_with("desired_state.rs")
                && violations[0].contains("before `mod tests`"),
            "got {violations:?}"
        );
    }

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mediaops-archtest-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }
}
