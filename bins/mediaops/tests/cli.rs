use serde_json::Value;
use std::process::Command;

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mediaops"));
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let runtime = std::env::temp_dir().join(format!(
        "mediaops-cli-xdg-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&runtime).expect("runtime");
    cmd.env("XDG_RUNTIME_DIR", &runtime)
        .env("XDG_STATE_HOME", &runtime);
    cmd
}

fn stdout_json(output: &std::process::Output) -> Value {
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "expected JSON: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert!(value.get("ok").is_none(), "output must be raw: {value}");
    value
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mediaops-cli-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("scratch");
    dir
}

const DS_ZERO_FREE: &str = "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n";

fn write_ds(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(&path, body).expect("config");
    path
}

#[test]
fn identity_supports_human_and_raw_json() {
    let human = bin().output().expect("identity");
    assert!(human.status.success());
    assert_eq!(
        String::from_utf8_lossy(&human.stdout).trim(),
        format!("mediaops {}", env!("CARGO_PKG_VERSION"))
    );
    for flags in [&["-o", "json"][..], &["--output=json"], &["-ojson"]] {
        let output = bin().args(flags).output().expect("JSON identity");
        assert!(output.status.success());
        let value = stdout_json(&output);
        assert_eq!(value["name"], "mediaops");
        assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);
    }
}

#[test]
fn parse_errors_honor_raw_json_anywhere_in_argv() {
    for flags in [
        vec!["-o", "json", "--nope"],
        vec!["--nope", "--output=json"],
        vec!["--nope", "-ojson"],
        vec!["--nope", "-o=json"],
    ] {
        let output = bin().args(flags).output().expect("usage");
        assert_eq!(output.status.code(), Some(2));
        let value = stdout_json(&output);
        assert_eq!(value["error"]["code"], "usage");
        assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);
    }
}

#[test]
fn human_usage_is_printed_once_on_stderr() {
    let output = bin().arg("--nope").output().expect("usage");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.matches("unexpected argument").count(), 1, "{stderr}");
}

#[test]
fn retired_commands_and_flags_fail_before_connecting_or_writing() {
    for args in [
        vec!["--json"],
        vec!["import-legacy"],
        vec!["plan"],
        vec!["run"],
        vec!["status", "--state-db", "/tmp/retired.db"],
        vec!["status", "--plans-dir", "/tmp/plans"],
        vec!["status", "--api-socket", "/tmp/api.sock"],
        vec!["why", "movie:tmdb:603", "--state-db", "/tmp/retired.db"],
        vec!["watch", "movie:tmdb:603", "--state-db", "/tmp/retired.db"],
        vec!["hold", "list", "--state-db", "/tmp/retired.db"],
        vec!["library", "reindex", "--state-db", "/tmp/retired.db"],
        vec![
            "library",
            "relocate",
            "--library-root",
            "/tmp/library",
            "--config",
            "/tmp/config.toml",
        ],
        vec![
            "library",
            "relocate",
            "--library-root",
            "/tmp/library",
            "--config-dir",
            "/tmp/config",
        ],
        vec![
            "library",
            "bootstrap",
            "--library-root",
            "/tmp/library",
            "--enable-timer",
        ],
        vec!["encode", "pause", "--state-db", "/tmp/retired.db"],
    ] {
        let output = bin().args(&args).output().expect("retired syntax");
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn invalid_output_is_rejected_for_every_command_before_work() {
    for command in [
        vec!["reconcile"],
        vec!["sync"],
        vec!["status"],
        vec!["list"],
        vec!["library", "reindex"],
        vec!["encode", "scan"],
        vec!["hold", "list"],
        vec!["reclaim", "apply"],
        vec!["doctor"],
    ] {
        let output = bin()
            .args(&command)
            .args(["-o", "yaml"])
            .output()
            .expect("invalid output");
        assert_eq!(output.status.code(), Some(2), "{command:?}");
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("started"));
    }
}

#[test]
fn maintenance_shares_one_exclusive_lock_and_json_suppresses_progress() {
    let dir = scratch("maintenance-lock");
    let state = dir.join("mediaops");
    std::fs::create_dir_all(&state).expect("state");
    let file = std::fs::File::create(state.join("mediaops.lock")).expect("lock");
    fs4::FileExt::try_lock(&file).expect("hold lock");
    let path = dir.to_str().expect("path");
    for args in [
        vec!["reclaim", "apply"],
        vec!["library", "reindex"],
        vec!["library", "bootstrap", "--library-root", path],
        vec!["library", "relocate", "--library-root", path],
        vec!["encode", "run"],
        vec!["new-machine", "export", "--out", path],
        vec![
            "new-machine",
            "import",
            "--from",
            path,
            "--library-root",
            path,
        ],
        vec![
            "pull",
            "--root",
            "movies",
            "--path",
            "movie.mkv",
            "--title-id",
            "movie:tmdb:603",
            "--name",
            "movie.mkv",
        ],
    ] {
        let output = bin()
            .env("XDG_STATE_HOME", &dir)
            .args(&args)
            .args(["-o", "json"])
            .output()
            .expect("locked maintenance");
        assert_eq!(
            output.status.code(),
            Some(3),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(stdout_json(&output)["error"]["code"], "lock_conflict");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("started"));
    }
    let preview = bin()
        .env("XDG_STATE_HOME", &dir)
        .args(["reclaim", "preview", "-o", "json"])
        .output()
        .expect("preview");
    assert_ne!(preview.status.code(), Some(3), "preview is lock-free");
    drop(file);
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn library_reindex_requires_home_and_leaves_local_catalog_untouched() {
    let dir = scratch("no-fallback");
    let library = dir.join("library");
    std::fs::create_dir_all(&library).expect("library");
    let output = bin()
        .env("XDG_STATE_HOME", &dir)
        .env("XDG_RUNTIME_DIR", &dir)
        .args(["library", "reindex", "--library-root"])
        .arg(&library)
        .args(["-o", "json"])
        .output()
        .expect("reindex");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout_json(&output)["error"]["code"], "runtime");
    assert!(!dir.join("mediaops/state.db").exists());
    std::fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn runtime_errors_are_readable_on_redirected_stderr() {
    let output = bin().arg("status").output().expect("status");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.lines().any(|line| line.starts_with("error: ")),
        "{stderr}"
    );
    assert!(stderr.contains("stopped"), "{stderr}");
}

#[test]
fn help_exits_ok() {
    let output = bin().arg("--help").output().expect("run mediaops --help");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(
        stdout.contains("Usage:"),
        "help must print clap usage, got: {stdout}"
    );
    assert!(
        stdout.contains("seedbox"),
        "help must mention seedbox: {stdout}"
    );
    for verb in [
        "get",
        "apply",
        "watch",
        "why",
        "status",
        "encode",
        "hold",
        "reclaim",
        "doctor",
        "new-machine",
        "sync",
    ] {
        assert!(stdout.contains(verb), "help must mention {verb}: {stdout}");
    }
    assert!(
        serde_json::from_str::<Value>(stdout.trim()).is_err(),
        "help must not be a JSON envelope: {stdout}"
    );
}

#[test]
fn doctor_repair_unattended_from_a_public_laptop() {
    let output = bin()
        .args(["-o", "json", "doctor", "--repair"])
        .output()
        .expect("doctor repair");
    assert_eq!(output.status.code(), Some(5));
    let value = stdout_json(&output);
    assert!(value.get("ok").is_none());
    assert_eq!(value["error"]["code"], "policy_refusal");
    let message = value["error"]["message"].as_str().unwrap_or("");
    assert!(
        message.contains("public laptop") || message.contains("unattended"),
        "got {message}"
    );
}

#[test]
fn seedbox_bootstrap_unimplemented_provider_fails_loudly() {
    let output = bin()
        .args([
            "-o",
            "json",
            "seedbox",
            "bootstrap",
            "--provider",
            "docker-compose",
        ])
        .output()
        .expect("run bootstrap unimplemented");
    assert_eq!(output.status.code(), Some(2));
    let value = stdout_json(&output);
    assert!(value.get("ok").is_none());
    let message = value["error"]["message"].as_str().unwrap_or("");
    assert!(
        message.contains("unimplemented") || message.contains("docker_compose"),
        "got {message}"
    );
}

#[test]
fn seedbox_bootstrap_without_yes_is_policy_refusal() {
    let dir = std::env::temp_dir().join(format!(
        "mediaops-cli-bootstrap-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let ssh = dir.join("ssh_config");
    std::fs::write(&ssh, "Host seedbox\n  HostName 127.0.0.1\n  User x\n").expect("ssh");
    let ds = dir.join("config.toml");
    std::fs::write(
        &ds,
        "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 1\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n",
    )
    .expect("ds");
    let output = bin()
        .args([
            "-o",
            "json",
            "seedbox",
            "bootstrap",
            "--provider",
            "already-there",
            "--config-dir",
            dir.to_str().unwrap(),
            "--config",
            ds.to_str().unwrap(),
            "--ssh-config",
            ssh.to_str().unwrap(),
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("run bootstrap plan");
    assert!(
        !dir.join("tls").exists(),
        "without --yes must not create tls/"
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(output.status.code(), Some(5));
    let value = stdout_json(&output);
    assert!(value.get("ok").is_none());
    assert_eq!(value["error"]["code"], "policy_refusal");
    let message = value["error"]["message"].as_str().unwrap_or("");
    let steps = value["report"]["steps"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    let haystack = format!("{message} {steps}");
    assert!(
        haystack.contains("mint") && haystack.contains("tls") && haystack.contains("fingerprint"),
        "plan must mention mint/tls/fingerprint, got {haystack}"
    );
    assert_eq!(value["report"]["applied"], false);
    assert!(
        value["report"]["steps"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0)
            > 0,
        "NeedsConfirm JSON must include BootstrapReport steps in data"
    );
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf8");
    assert_eq!(
        stdout.trim().lines().count(),
        1,
        "must not double-emit envelopes: {stdout}"
    );
}

#[test]
fn seedbox_bootstrap_git_work_tree_is_policy_refusal() {
    let dir = std::env::temp_dir().join(format!(
        "mediaops-cli-git-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(dir.join(".git")).expect("mkdir");
    let ssh = dir.join("ssh_config");
    std::fs::write(&ssh, "Host seedbox\n  HostName 127.0.0.1\n  User x\n").expect("ssh");
    let ds = dir.join("config.toml");
    std::fs::write(
        &ds,
        "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 1\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n",
    )
    .expect("ds");
    let output = bin()
        .args([
            "-o",
            "json",
            "seedbox",
            "bootstrap",
            "--provider",
            "already-there",
            "--yes",
            "--config-dir",
            dir.to_str().unwrap(),
            "--config",
            ds.to_str().unwrap(),
            "--ssh-config",
            ssh.to_str().unwrap(),
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
            "--skip-probe",
        ])
        .output()
        .expect("run bootstrap git");
    assert!(
        !dir.join("tls").join("ca.pem").exists(),
        "git work tree must refuse before mint"
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(output.status.code(), Some(5));
    let value = stdout_json(&output);
    assert!(value.get("ok").is_none());
    assert_eq!(value["error"]["code"], "policy_refusal");
    let message = value["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("git"), "got {message}");
}

#[test]
fn version_exits_ok() {
    let output = bin()
        .arg("--version")
        .output()
        .expect("run mediaops --version");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(
        stdout,
        format!("mediaops {}\n", env!("CARGO_PKG_VERSION")),
        "version must print the exact package version"
    );
    assert!(
        serde_json::from_str::<Value>(stdout.trim()).is_err(),
        "version must not be a JSON envelope: {stdout}"
    );
}

#[test]
fn reclaim_preview_rejects_max_and_desired_state() {
    for extra in [["--max", "1"], ["--config", "/tmp/x"]] {
        let output = bin()
            .args(["-o", "json", "reclaim", "preview"])
            .args(extra)
            .output()
            .expect("preview extra");
        assert_eq!(
            output.status.code(),
            Some(2),
            "preview extra {:?} stdout={} stderr={}",
            extra,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value = stdout_json(&output);
        assert_eq!(value["error"]["code"], "usage", "{extra:?}");
    }
}

#[test]
fn new_machine_import_git_work_tree_is_exit_5() {
    let bundle = scratch("nm-git-bundle");
    std::fs::write(bundle.join("config.toml"), DS_ZERO_FREE).expect("ds");
    std::fs::write(bundle.join("title-index.json"), b"[]").expect("index");
    std::fs::create_dir_all(bundle.join("tls")).expect("tls");
    std::fs::write(bundle.join("tls/ca.pem"), b"ca").expect("pem");
    let dest = scratch("nm-git-dest");
    std::fs::create_dir_all(dest.join(".git")).expect("git");
    let output = bin()
        .args([
            "-o",
            "json",
            "new-machine",
            "import",
            "--from",
            bundle.to_str().unwrap(),
            "--library-root",
            dest.join("library").to_str().unwrap(),
            "--config-dir",
            dest.to_str().unwrap(),
        ])
        .output()
        .expect("import git");
    assert_eq!(output.status.code(), Some(5));
    let value = stdout_json(&output);
    assert_eq!(value["error"]["code"], "policy_refusal");
    assert!(!dest.join("config.toml").exists());
    assert!(!dest.join("tls/ca.pem").exists());
    let _ = std::fs::remove_dir_all(&bundle);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn new_machine_import_tls_dir_git_work_tree_is_exit_5() {
    let bundle = scratch("nm-tls-git-bundle");
    std::fs::write(bundle.join("config.toml"), DS_ZERO_FREE).expect("ds");
    std::fs::write(bundle.join("title-index.json"), b"[]").expect("index");
    std::fs::create_dir_all(bundle.join("tls")).expect("tls");
    std::fs::write(bundle.join("tls/ca.pem"), b"ca").expect("pem");
    let dest = scratch("nm-tls-git-dest");
    let git_root = scratch("nm-tls-git-tree");
    std::fs::create_dir_all(git_root.join(".git")).expect("git");
    let tls_dir = git_root.join("tls");
    std::fs::create_dir_all(&tls_dir).expect("tls dest");
    let output = bin()
        .args([
            "-o",
            "json",
            "new-machine",
            "import",
            "--from",
            bundle.to_str().unwrap(),
            "--library-root",
            dest.join("library").to_str().unwrap(),
            "--config-dir",
            dest.to_str().unwrap(),
            "--tls-dir",
            tls_dir.to_str().unwrap(),
        ])
        .output()
        .expect("import tls git");
    assert_eq!(output.status.code(), Some(5));
    let value = stdout_json(&output);
    assert_eq!(value["error"]["code"], "policy_refusal");
    assert!(!tls_dir.join("ca.pem").exists());
    assert!(!dest.join("config.toml").exists());
    let _ = std::fs::remove_dir_all(&bundle);
    let _ = std::fs::remove_dir_all(&dest);
    let _ = std::fs::remove_dir_all(&git_root);
}

#[test]
fn new_machine_export_git_work_tree_is_exit_5() {
    let src = scratch("nm-export-src");
    write_ds(src.as_path(), DS_ZERO_FREE);
    let out = scratch("nm-export-git");
    std::fs::create_dir_all(out.join(".git")).expect("git");
    let output = bin()
        .args([
            "-o",
            "json",
            "new-machine",
            "export",
            "--out",
            out.to_str().unwrap(),
            "--config-dir",
            src.to_str().unwrap(),
        ])
        .output()
        .expect("export git");
    assert_eq!(output.status.code(), Some(5));
    let value = stdout_json(&output);
    assert_eq!(value["error"]["code"], "policy_refusal");
    assert!(!out.join("config.toml").exists());
    assert!(!out.join("tls/ca.pem").exists());
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&out);
}
