use std::process::Command;

use serde_json::Value;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mediaops"))
}

fn stdout_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("stdout must be one JSON envelope")
}

fn assert_json_tracing_stderr(stderr: &str) {
    assert!(!stderr.is_empty(), "tracing must land on stderr");
    for line in stderr.lines().filter(|line| !line.is_empty()) {
        let event: Value = serde_json::from_str(line).unwrap_or_else(|_| {
            panic!("stderr must be JSON tracing lines when not a tty, got: {line}")
        });
        assert!(
            event.get("ok").is_none(),
            "result envelope must not appear on stderr: {stderr}"
        );
    }
}

fn assert_no_result_envelope_on_stderr(stderr: &str) {
    assert!(
        !stderr.contains(r#""ok":false"#) && !stderr.contains(r#""ok": false"#),
        "result envelope must not appear on stderr: {stderr}"
    );
}

#[test]
fn json_happy() {
    let output = bin().arg("--json").output().expect("run mediaops --json");
    assert_eq!(output.status.code(), Some(0));
    let value = stdout_json(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["name"], "mediaops");
    assert_eq!(value["data"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(value.get("error"), Some(&Value::Null));
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf8");
    assert_eq!(stdout.trim().lines().count(), 1);
    assert_json_tracing_stderr(&String::from_utf8_lossy(&output.stderr));
}

#[test]
fn human_happy() {
    let output = bin().output().expect("run mediaops");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert_eq!(
        stdout.trim(),
        format!("mediaops {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(
        serde_json::from_str::<Value>(stdout.trim()).is_err(),
        "human stdout must not be JSON"
    );
    assert_json_tracing_stderr(&String::from_utf8_lossy(&output.stderr));
}

#[test]
fn usage_unknown_flag() {
    let output = bin()
        .arg("--nope-not-a-flag")
        .output()
        .expect("run mediaops unknown flag");
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(
        stdout.trim().is_empty(),
        "human usage must not print a result envelope on stdout: {stdout}"
    );
    assert_no_result_envelope_on_stderr(&String::from_utf8_lossy(&output.stderr));
}

#[test]
fn usage_unknown_flag_with_json() {
    let output = bin()
        .args(["--json", "--nope-not-a-flag"])
        .output()
        .expect("run mediaops --json unknown flag");
    assert_eq!(output.status.code(), Some(2));
    let value = stdout_json(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value.get("data"), Some(&Value::Null));
    assert_eq!(value["error"]["code"], "usage");
    assert_no_result_envelope_on_stderr(&String::from_utf8_lossy(&output.stderr));
}

#[test]
fn usage_unknown_flag_json_anywhere_in_argv() {
    let output = bin()
        .args(["--nope-not-a-flag", "--json"])
        .output()
        .expect("run mediaops unknown flag --json");
    assert_eq!(output.status.code(), Some(2));
    let value = stdout_json(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value.get("data"), Some(&Value::Null));
    assert_eq!(value["error"]["code"], "usage");
    assert_no_result_envelope_on_stderr(&String::from_utf8_lossy(&output.stderr));
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
        "plan",
        "run",
        "watch",
        "why",
        "status",
        "encode",
        "hold",
        "reclaim",
        "doctor",
        "new-machine",
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
        .args(["--json", "doctor", "--repair"])
        .output()
        .expect("doctor repair");
    assert_eq!(output.status.code(), Some(5));
    let value = stdout_json(&output);
    assert_eq!(value["ok"], false);
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
            "--json",
            "seedbox",
            "bootstrap",
            "--provider",
            "docker-compose",
        ])
        .output()
        .expect("run bootstrap unimplemented");
    assert_eq!(output.status.code(), Some(2));
    let value = stdout_json(&output);
    assert_eq!(value["ok"], false);
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
    let ds = dir.join("desired-state.toml");
    std::fs::write(
        &ds,
        "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 1\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n",
    )
    .expect("ds");
    let output = bin()
        .args([
            "--json",
            "seedbox",
            "bootstrap",
            "--provider",
            "already-there",
            "--config-dir",
            dir.to_str().unwrap(),
            "--desired-state",
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
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "policy_refusal");
    let message = value["error"]["message"].as_str().unwrap_or("");
    let steps = value["data"]["steps"]
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
    assert_eq!(value["data"]["applied"], false);
    assert!(
        value["data"]["steps"]
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
    let ds = dir.join("desired-state.toml");
    std::fs::write(
        &ds,
        "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 1\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n",
    )
    .expect("ds");
    let output = bin()
        .args([
            "--json",
            "seedbox",
            "bootstrap",
            "--provider",
            "already-there",
            "--yes",
            "--config-dir",
            dir.to_str().unwrap(),
            "--desired-state",
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
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "policy_refusal");
    let message = value["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("git"), "got {message}");
}

#[test]
fn library_bootstrap_creates_schema_dirs() {
    let dir = std::env::temp_dir().join(format!(
        "mediaops-cli-lib-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let ds = dir.join("desired-state.toml");
    std::fs::write(
        &ds,
        "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n",
    )
    .expect("ds");
    let lib = dir.join("library");
    let units = dir.join("units");
    let output = bin()
        .args([
            "--json",
            "library",
            "bootstrap",
            "--library-root",
            lib.to_str().unwrap(),
            "--desired-state",
            ds.to_str().unwrap(),
            "--config-dir",
            dir.to_str().unwrap(),
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
            "--unit-dir",
            units.to_str().unwrap(),
        ])
        .output()
        .expect("run library bootstrap");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "library bootstrap failed: {stderr}"
    );
    for name in ["movies", "series", "music", "_ops", "_incoming"] {
        assert!(lib.join(name).is_dir(), "{name}");
    }
    assert!(units.join("mediaops-run.service").is_file());
    assert!(units.join("mediaops-run.timer").is_file());
    assert!(units.join("mediaopsd-home.service").is_file());
    let home = std::fs::read_to_string(units.join("mediaopsd-home.service")).expect("home");
    assert!(home.contains("Restart=on-failure"), "{home}");
    let exec = home
        .lines()
        .find(|l| l.starts_with("ExecStart="))
        .expect("ExecStart");
    assert!(
        exec.contains("mediaopsd") && exec.contains("serve --role home"),
        "ExecStart must run mediaopsd serve --role home, got {exec}"
    );
    let timer = std::fs::read_to_string(units.join("mediaops-run.timer")).expect("timer");
    assert!(timer.contains("OnUnitInactiveSec="));
    assert!(timer.contains("OnBootSec="));
    assert!(!timer.contains("OnCalendar"));
    let service = std::fs::read_to_string(units.join("mediaops-run.service")).expect("service");
    assert!(
        service.contains(" run --state-db "),
        "ExecStart must pin --state-db *after* the verb (clap scopes it to `run`), got {service}"
    );
    assert!(service.contains("TimeoutStartSec=infinity"));
    assert!(
        service.contains("Nice=10"),
        "spinning-disk niceness: {service}"
    );
    let value = stdout_json(&output);
    assert_eq!(value["ok"], true);
    let root = value["data"]["library_root"]
        .as_str()
        .expect("library_root");
    assert!(
        std::path::Path::new(root).is_absolute(),
        "library_root must be canonical, got {root}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn version_exits_ok() {
    let output = bin()
        .arg("--version")
        .output()
        .expect("run mediaops --version");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(
        stdout.contains("mediaops") && stdout.contains(env!("CARGO_PKG_VERSION")),
        "version must print clap version, got: {stdout}"
    );
    assert!(
        serde_json::from_str::<Value>(stdout.trim()).is_err(),
        "version must not be a JSON envelope: {stdout}"
    );
}

#[test]
fn usage_json_equals_true() {
    let output = bin()
        .args(["--json=true", "--nope-not-a-flag"])
        .output()
        .expect("run mediaops --json=true unknown flag");
    assert_eq!(output.status.code(), Some(2));
    let value = stdout_json(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value.get("data"), Some(&Value::Null));
    assert_eq!(value["error"]["code"], "usage");
    assert_no_result_envelope_on_stderr(&String::from_utf8_lossy(&output.stderr));
}

#[test]
fn usage_json_equals_false_stays_human() {
    let output = bin()
        .args(["--json=false", "--nope-not-a-flag"])
        .output()
        .expect("run mediaops --json=false unknown flag");
    assert_eq!(output.status.code(), Some(2));
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(
        stdout.trim().is_empty(),
        "human usage must not print a result envelope on stdout: {stdout}"
    );
    assert_no_result_envelope_on_stderr(&String::from_utf8_lossy(&output.stderr));
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
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

#[test]
fn lock_conflict_is_exit_3_never_silent_0() {
    let dir = scratch("lock-conflict");
    let lock_path = dir.join("mediaops.lock");
    let file = std::fs::File::create(&lock_path).expect("lock file");
    fs4::FileExt::try_lock(&file).expect("hold lock");
    let output = bin()
        .args([
            "--json",
            "run",
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("run");
    drop(file);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        output.status.code(),
        Some(3),
        "lock conflict must not be silent 0: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value = stdout_json(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "lock_conflict");
}

#[test]
fn watch_is_lock_free_and_json_envelope() {
    let dir = scratch("watch");
    let lock_path = dir.join("mediaops.lock");
    let file = std::fs::File::create(&lock_path).expect("lock file");
    fs4::FileExt::try_lock(&file).expect("hold lock");
    let output = bin()
        .args([
            "--json",
            "watch",
            "movie:tmdb:603",
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("watch");
    drop(file);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "watch must be lock-free: {stderr}"
    );
    let value = stdout_json(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["title_id"], "movie:tmdb:603");
    assert_eq!(value["data"]["created"], true);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reclaim_preview_is_lock_free_and_apply_is_exclusive() {
    let dir = scratch("reclaim-lock");
    let lock_path = dir.join("mediaops.lock");
    let file = std::fs::File::create(&lock_path).expect("lock file");
    fs4::FileExt::try_lock(&file).expect("hold lock");
    let preview = bin()
        .args([
            "--json",
            "reclaim",
            "preview",
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
            "--config-dir",
            dir.to_str().unwrap(),
        ])
        .output()
        .expect("preview");
    let apply = bin()
        .args([
            "--json",
            "reclaim",
            "apply",
            "--max",
            "1",
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
            "--config-dir",
            dir.to_str().unwrap(),
        ])
        .output()
        .expect("apply");
    drop(file);
    let _ = std::fs::remove_dir_all(&dir);
    // Preview is lock-free; UDS may be down (runtime) but must not be lock_conflict.
    assert_ne!(
        preview.status.code(),
        Some(3),
        "reclaim preview must be lock-free: stdout={} stderr={}",
        String::from_utf8_lossy(&preview.stdout),
        String::from_utf8_lossy(&preview.stderr)
    );
    assert_eq!(
        apply.status.code(),
        Some(3),
        "reclaim apply must take exclusive flock: stdout={} stderr={}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );
    let value = stdout_json(&apply);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "lock_conflict");
}

#[test]
fn reclaim_preview_rejects_max_and_desired_state() {
    for extra in [["--max", "1"], ["--desired-state", "/tmp/x"]] {
        let output = bin()
            .args(["--json", "reclaim", "preview"])
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
fn why_and_status_json_envelopes() {
    let dir = scratch("why-status");
    let db = dir.join("state.db");
    let watch = bin()
        .args([
            "--json",
            "watch",
            "movie:tmdb:603",
            "--state-db",
            db.to_str().unwrap(),
        ])
        .output()
        .expect("watch");
    assert_eq!(watch.status.code(), Some(0));
    let why = bin()
        .args([
            "--json",
            "why",
            "movie:tmdb:603",
            "--state-db",
            db.to_str().unwrap(),
            "--config-dir",
            dir.to_str().unwrap(),
        ])
        .output()
        .expect("why");
    assert_eq!(
        why.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&why.stderr)
    );
    let why_v = stdout_json(&why);
    assert_eq!(why_v["ok"], true);
    assert_eq!(why_v["data"]["title_id"], "movie:tmdb:603");
    assert_eq!(why_v["data"]["want"]["state"], "open");
    assert_eq!(why_v["data"]["want"]["title_id"], "movie:tmdb:603");
    assert_eq!(why_v["data"]["grab"], Value::Null);
    assert_eq!(why_v["data"]["import"], Value::Null);
    assert_eq!(why_v["data"]["hold"], Value::Null);
    assert_eq!(why_v["data"]["df"], Value::Null);
    assert_eq!(why_v["data"]["reclaim"], Value::Null);
    // A dead socket: this machine may well have a live home gateway on the
    // default one, and `df` must be judged against no daemon.
    let dead_socket = dir.join("no-such.sock");
    let status = bin()
        .args([
            "--json",
            "status",
            "--state-db",
            db.to_str().unwrap(),
            "--plans-dir",
            dir.join("plans").to_str().unwrap(),
            "--socket",
            dead_socket.to_str().unwrap(),
            "--tls-dir",
            dir.to_str().unwrap(),
        ])
        .output()
        .expect("status");
    assert_eq!(status.status.code(), Some(0));
    let status_v = stdout_json(&status);
    assert_eq!(status_v["ok"], true);
    assert_eq!(status_v["data"]["open_wants"][0]["state"], "open");
    assert_eq!(
        status_v["data"]["open_wants"][0]["title_id"],
        "movie:tmdb:603"
    );
    assert!(status_v["data"]["watermark"].is_object());
    assert_eq!(status_v["data"]["df"], Value::Null);
    assert_eq!(status_v["data"]["reclaim"], Value::Null);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn status_and_why_report_lock_holder_when_flock_is_held() {
    let dir = scratch("lock-holder");
    let db = dir.join("state.db");
    let lock_path = dir.join("mediaops.lock");
    std::fs::write(
        &lock_path,
        r#"{"pid":4242,"started_at":1,"command":"mediaops run"}
"#,
    )
    .expect("write lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    fs4::FileExt::try_lock(&file).expect("hold lock");
    let status = bin()
        .args([
            "--json",
            "status",
            "--state-db",
            db.to_str().unwrap(),
            "--plans-dir",
            dir.join("plans").to_str().unwrap(),
        ])
        .output()
        .expect("status");
    assert_eq!(
        status.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_v = stdout_json(&status);
    assert_eq!(status_v["ok"], true);
    assert_eq!(status_v["data"]["lock"]["pid"], 4242);
    assert_eq!(status_v["data"]["lock"]["command"], "mediaops run");
    let why = bin()
        .args([
            "--json",
            "why",
            "movie:tmdb:603",
            "--state-db",
            db.to_str().unwrap(),
            "--config-dir",
            dir.to_str().unwrap(),
        ])
        .output()
        .expect("why");
    assert_eq!(
        why.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&why.stderr)
    );
    let why_v = stdout_json(&why);
    assert_eq!(why_v["data"]["lock"]["pid"], 4242);
    assert_eq!(why_v["data"]["df"], Value::Null);
    drop(file);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn why_and_status_df_from_seedbox_on_loopback_lock_free() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    rt.block_on(async {
        let _g = HOLD_NET.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("why-df");
        let lb = start_hold_loopback().await;
        let lock_path = dir.join("mediaops.lock");
        let file = std::fs::File::create(&lock_path).expect("lock file");
        fs4::FileExt::try_lock(&file).expect("hold lock");
        let why = bin()
            .args([
                "--json",
                "why",
                "movie:tmdb:603",
                "--state-db",
                dir.join("state.db").to_str().unwrap(),
                "--config-dir",
                dir.to_str().unwrap(),
                "--socket",
                lb.sock.to_str().unwrap(),
                "--tls-dir",
                lb.tls_dir.to_str().unwrap(),
            ])
            .output()
            .expect("why");
        assert_eq!(
            why.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&why.stderr)
        );
        let why_v = stdout_json(&why);
        assert_eq!(why_v["ok"], true);
        assert!(why_v["data"]["df"]["free"].as_u64().is_some(), "{why_v}");
        assert_eq!(why_v["data"]["grab"], Value::Null);
        let status = bin()
            .args([
                "--json",
                "status",
                "--state-db",
                dir.join("state.db").to_str().unwrap(),
                "--plans-dir",
                dir.join("plans").to_str().unwrap(),
                "--socket",
                lb.sock.to_str().unwrap(),
                "--tls-dir",
                lb.tls_dir.to_str().unwrap(),
            ])
            .output()
            .expect("status");
        assert_eq!(
            status.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        let status_v = stdout_json(&status);
        assert!(
            status_v["data"]["df"]["free"].as_u64().is_some(),
            "{status_v}"
        );
        drop(file);
        drop(lb);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn watch_second_call_reuses_open_want() {
    let dir = scratch("watch-idem");
    let db = dir.join("state.db");
    let first = bin()
        .args([
            "--json",
            "watch",
            "movie:tmdb:603",
            "--state-db",
            db.to_str().unwrap(),
        ])
        .output()
        .expect("watch");
    assert_eq!(first.status.code(), Some(0));
    let first_v = stdout_json(&first);
    assert_eq!(first_v["data"]["created"], true);
    let second = bin()
        .args([
            "--json",
            "watch",
            "movie:tmdb:603",
            "--state-db",
            db.to_str().unwrap(),
        ])
        .output()
        .expect("watch2");
    assert_eq!(second.status.code(), Some(0));
    let second_v = stdout_json(&second);
    assert_eq!(second_v["data"]["created"], false);
    assert_eq!(second_v["data"]["job_id"], first_v["data"]["job_id"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn encode_pause_is_lock_free_json_envelope() {
    let dir = scratch("encode-pause");
    let lock_path = dir.join("mediaops.lock");
    let file = std::fs::File::create(&lock_path).expect("lock file");
    fs4::FileExt::try_lock(&file).expect("hold lock");
    let output = bin()
        .args([
            "--json",
            "encode",
            "pause",
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("pause");
    drop(file);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = stdout_json(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["encode_pause"], true);
    let off = bin()
        .args([
            "--json",
            "encode",
            "pause",
            "--off",
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("pause off");
    assert_eq!(off.status.code(), Some(0));
    let off_v = stdout_json(&off);
    assert_eq!(off_v["data"]["encode_pause"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn hold_help_mentions_list() {
    let output = bin().args(["hold", "--help"]).output().expect("hold help");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    assert!(
        stdout.contains("list"),
        "hold help must mention list: {stdout}"
    );
    assert!(
        stdout.contains("approve"),
        "hold help must mention approve: {stdout}"
    );
    assert!(
        stdout.contains("reject"),
        "hold help must mention reject: {stdout}"
    );
    assert!(
        !stdout.to_ascii_lowercase().contains("research"),
        "omit hold research: {stdout}"
    );
}

#[test]
fn hold_list_lock_free_json_empty_on_loopback() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    rt.block_on(async {
        let _g = HOLD_NET.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("hold-cli");
        let library = dir.join("library");
        for name in ["movies", "series", "music", "_incoming"] {
            std::fs::create_dir_all(library.join(name)).expect("layout");
        }
        let lb = start_hold_loopback().await;
        let lock_path = dir.join("mediaops.lock");
        let file = std::fs::File::create(&lock_path).expect("lock file");
        fs4::FileExt::try_lock(&file).expect("hold lock");
        let output = bin()
            .args([
                "--json",
                "hold",
                "list",
                "--socket",
                lb.sock.to_str().unwrap(),
                "--tls-dir",
                lb.tls_dir.to_str().unwrap(),
                "--state-db",
                dir.join("state.db").to_str().unwrap(),
            ])
            .output()
            .expect("hold list");
        drop(file);
        assert_eq!(
            output.status.code(),
            Some(0),
            "hold list must be lock-free (holds rotting as a junk drawer): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value = stdout_json(&output);
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["holds"], serde_json::json!([]));
        for name in ["movies", "series", "music", "_incoming"] {
            let empty = std::fs::read_dir(library.join(name))
                .expect("read")
                .next()
                .is_none();
            assert!(empty, "{name} must not become a hold folder");
        }
        drop(lb);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

#[test]
fn hold_approve_reject_unknown_key_is_usage_lock_free() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("rt");
    rt.block_on(async {
        let _g = HOLD_NET.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch("hold-cli-decide");
        let library = dir.join("library");
        for name in ["movies", "series", "music", "_incoming"] {
            std::fs::create_dir_all(library.join(name)).expect("layout");
        }
        let lb = start_hold_loopback().await;
        let lock_path = dir.join("mediaops.lock");
        let file = std::fs::File::create(&lock_path).expect("lock file");
        fs4::FileExt::try_lock(&file).expect("hold lock");
        for verb in ["approve", "reject"] {
            let output = bin()
                .args([
                    "--json",
                    "hold",
                    verb,
                    "movie:tmdb:603",
                    "deadbeef",
                    "--socket",
                    lb.sock.to_str().unwrap(),
                    "--tls-dir",
                    lb.tls_dir.to_str().unwrap(),
                    "--state-db",
                    dir.join("state.db").to_str().unwrap(),
                ])
                .output()
                .expect(verb);
            assert_eq!(
                output.status.code(),
                Some(2),
                "{verb} unknown key / grabber=none is usage, not lock conflict: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        drop(file);
        for name in ["movies", "series", "music", "_incoming"] {
            let empty = std::fs::read_dir(library.join(name))
                .expect("read")
                .next()
                .is_none();
            assert!(empty, "{name} must not be written by hold approve/reject");
        }
        drop(lb);
        let _ = std::fs::remove_dir_all(&dir);
    });
}

static HOLD_NET: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct HoldLoopback {
    sock: std::path::PathBuf,
    tls_dir: std::path::PathBuf,
    remote_root: std::path::PathBuf,
    seed_task: tokio::task::JoinHandle<()>,
    uds_task: tokio::task::JoinHandle<()>,
}

impl Drop for HoldLoopback {
    fn drop(&mut self) {
        self.seed_task.abort();
        self.uds_task.abort();
        let _ = std::fs::remove_file(&self.sock);
        let _ = std::fs::remove_dir_all(&self.tls_dir);
        let _ = std::fs::remove_dir_all(&self.remote_root);
    }
}

async fn start_hold_loopback() -> HoldLoopback {
    use mediaops_core::{Allowlist, Grabber, UnderlayMode, endpoint_fingerprint};
    use mediaops_transfer::{
        HomeGateway, Seedbox, connect_home, connect_tcp, mint, serve_home_unix, serve_tcp,
    };
    use tokio::net::{TcpListener, UnixListener};

    let remote_root = scratch("hold-remote");
    let id = mint().expect("mint");
    let tls_dir = scratch("hold-tls");
    id.write_to_dir(&tls_dir).expect("write tls");
    let tcp = TcpListener::bind("127.0.0.1:0").await.expect("bind tcp");
    let addr = tcp.local_addr().expect("addr");
    let mut allowlist = Allowlist::new();
    allowlist
        .add_root("seedbox", remote_root.clone())
        .expect("root");
    let seed = Seedbox::new(allowlist, "0.1.0", Grabber::None);
    let server = id.server_config().expect("server");
    let seed_task = tokio::spawn(async move {
        let _ = serve_tcp(tcp, server, seed).await;
    });
    let client = id.client_config().expect("client");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match connect_tcp(addr, client.clone()).await {
            Ok(_) => break,
            Err(err) => {
                if tokio::time::Instant::now() >= deadline {
                    panic!("tcp connect: {err}");
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
    }
    let fingerprint = endpoint_fingerprint(&addr.to_string(), UnderlayMode::Direct);
    let gateway = HomeGateway::connect(addr, client, fingerprint, 1)
        .await
        .expect("gw");
    let sock = scratch("hold-uds").join("mediaops.sock");
    let unix = UnixListener::bind(&sock).expect("bind uds");
    let uds_server = id.server_config().expect("server");
    let uds_task = tokio::spawn(async move {
        let _ = serve_home_unix(unix, uds_server, gateway).await;
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match connect_home(&sock, &tls_dir).await {
            Ok(_) => break,
            Err(err) => {
                if tokio::time::Instant::now() >= deadline {
                    panic!("home gateway: {err}");
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
    }
    HoldLoopback {
        sock,
        tls_dir,
        remote_root,
        seed_task,
        uds_task,
    }
}

const DS_ZERO_FREE: &str = "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 0\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n";

fn write_ds(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let ds = dir.join("desired-state.toml");
    std::fs::write(&ds, body).expect("ds");
    ds
}

#[test]
fn library_relocate_rewrites_root_and_units_without_copying_media() {
    let dir = scratch("relocate-happy");
    let ds = write_ds(dir.as_path(), DS_ZERO_FREE);
    let old = dir.join("old");
    let neu = dir.join("new");
    let units = dir.join("units");
    let db = dir.join("state.db");
    let rel = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
    std::fs::create_dir_all(old.join("movies/The.Matrix.(1999)")).expect("mkdir");
    std::fs::write(old.join(rel), b"orig").expect("media");
    let boot = bin()
        .args([
            "--json",
            "library",
            "bootstrap",
            "--library-root",
            old.to_str().unwrap(),
            "--desired-state",
            ds.to_str().unwrap(),
            "--config-dir",
            dir.to_str().unwrap(),
            "--state-db",
            db.to_str().unwrap(),
            "--unit-dir",
            units.to_str().unwrap(),
        ])
        .output()
        .expect("bootstrap");
    assert_eq!(
        boot.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&boot.stderr)
    );
    let reindex = bin()
        .args([
            "--json",
            "library",
            "reindex",
            "--library-root",
            old.to_str().unwrap(),
            "--state-db",
            db.to_str().unwrap(),
        ])
        .output()
        .expect("reindex");
    assert_eq!(
        reindex.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&reindex.stderr)
    );
    let output = bin()
        .args([
            "--json",
            "library",
            "relocate",
            "--library-root",
            neu.to_str().unwrap(),
            "--desired-state",
            ds.to_str().unwrap(),
            "--config-dir",
            dir.to_str().unwrap(),
            "--state-db",
            db.to_str().unwrap(),
            "--unit-dir",
            units.to_str().unwrap(),
        ])
        .output()
        .expect("relocate");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "relocate failed: {stderr}");
    let value = stdout_json(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(
        value["data"]["rewritten_absolute"], 0,
        "relative title-index paths must stay relative"
    );
    let root = value["data"]["library_root"].as_str().expect("root");
    assert!(std::path::Path::new(root).is_absolute(), "{root}");
    for name in ["movies", "series", "music", "_ops", "_incoming"] {
        assert!(neu.join(name).is_dir(), "{name}");
    }
    assert!(old.join(rel).is_file(), "must not move media");
    assert!(!neu.join(rel).exists(), "must not copy media");
    let timer = std::fs::read_to_string(units.join("mediaops-run.timer")).expect("timer");
    assert!(timer.contains("OnUnitInactiveSec="));
    assert!(!timer.contains("OnCalendar"));
    assert!(units.join("mediaops-run.service").is_file());
    assert!(units.join("mediaopsd-home.service").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn library_relocate_watermark_is_exit_5_without_store_or_units() {
    let dir = scratch("relocate-water");
    write_ds(
        dir.as_path(),
        "schema_version = 1\nmax_copy_gib = 1\nmin_free_gib = 999999999\nrange_len_mib = 8\nmax_nvenc = 1\nlock = false\n",
    );
    let units = dir.join("units");
    let output = bin()
        .args([
            "--json",
            "library",
            "relocate",
            "--library-root",
            dir.join("new").to_str().unwrap(),
            "--desired-state",
            dir.join("desired-state.toml").to_str().unwrap(),
            "--config-dir",
            dir.to_str().unwrap(),
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
            "--unit-dir",
            units.to_str().unwrap(),
        ])
        .output()
        .expect("relocate water");
    assert_eq!(output.status.code(), Some(5));
    let value = stdout_json(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "policy_refusal");
    assert!(!dir.join("state.db").exists(), "no store write");
    assert!(
        !units.join("mediaops-run.service").exists(),
        "no unit write"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn library_and_new_machine_are_exclusive_flock() {
    let dir = scratch("relocate-lock");
    write_ds(dir.as_path(), DS_ZERO_FREE);
    let lock_path = dir.join("mediaops.lock");
    let file = std::fs::File::create(&lock_path).expect("lock file");
    fs4::FileExt::try_lock(&file).expect("hold lock");
    let relocate = bin()
        .args([
            "--json",
            "library",
            "relocate",
            "--library-root",
            dir.join("new").to_str().unwrap(),
            "--desired-state",
            dir.join("desired-state.toml").to_str().unwrap(),
            "--config-dir",
            dir.to_str().unwrap(),
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
            "--unit-dir",
            dir.join("units").to_str().unwrap(),
        ])
        .output()
        .expect("relocate lock");
    let reindex = bin()
        .args([
            "--json",
            "library",
            "reindex",
            "--library-root",
            dir.join("lib").to_str().unwrap(),
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("reindex lock");
    let export = bin()
        .args([
            "--json",
            "new-machine",
            "export",
            "--out",
            dir.join("bundle").to_str().unwrap(),
            "--config-dir",
            dir.to_str().unwrap(),
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("export lock");
    let import = bin()
        .args([
            "--json",
            "new-machine",
            "import",
            "--from",
            dir.join("bundle").to_str().unwrap(),
            "--library-root",
            dir.join("lib").to_str().unwrap(),
            "--config-dir",
            dir.to_str().unwrap(),
            "--state-db",
            dir.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("import lock");
    drop(file);
    let _ = std::fs::remove_dir_all(&dir);
    for (name, output) in [
        ("relocate", relocate),
        ("reindex", reindex),
        ("export", export),
        ("import", import),
    ] {
        assert_eq!(
            output.status.code(),
            Some(3),
            "{name} must take exclusive flock: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value = stdout_json(&output);
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "lock_conflict");
    }
}

#[test]
fn new_machine_export_import_round_trip_on_empty_home() {
    let src = scratch("nm-src");
    let ds = write_ds(src.as_path(), DS_ZERO_FREE);
    let tls = src.join("tls");
    std::fs::create_dir_all(&tls).expect("tls");
    std::fs::write(tls.join("ca.pem"), b"ca-bytes").expect("pem");
    std::fs::write(tls.join("client.key"), b"key-bytes").expect("key");
    let lib = src.join("library");
    let rel = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
    std::fs::create_dir_all(lib.join("movies/The.Matrix.(1999)")).expect("mkdir");
    std::fs::write(lib.join(rel), b"orig").expect("media");
    let src_db = src.join("state.db");
    let reindex = bin()
        .args([
            "--json",
            "library",
            "reindex",
            "--library-root",
            lib.to_str().unwrap(),
            "--state-db",
            src_db.to_str().unwrap(),
        ])
        .output()
        .expect("reindex");
    assert_eq!(
        reindex.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&reindex.stderr)
    );
    let reindex_v = stdout_json(&reindex);
    assert_eq!(reindex_v["data"]["indexed"], 1);

    let bundle = scratch("nm-bundle");
    let export = bin()
        .args([
            "--json",
            "new-machine",
            "export",
            "--out",
            bundle.to_str().unwrap(),
            "--config-dir",
            src.to_str().unwrap(),
            "--desired-state",
            ds.to_str().unwrap(),
            "--tls-dir",
            tls.to_str().unwrap(),
            "--state-db",
            src_db.to_str().unwrap(),
        ])
        .output()
        .expect("export");
    assert_eq!(
        export.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&export.stderr)
    );
    assert!(bundle.join("desired-state.toml").is_file());
    assert!(bundle.join("title-index.json").is_file());
    assert!(bundle.join("tls/ca.pem").is_file());
    assert!(bundle.join("tls/client.key").is_file());
    let index: Value =
        serde_json::from_slice(&std::fs::read(bundle.join("title-index.json")).expect("index"))
            .expect("index json");
    assert_eq!(index[0]["title_id"], "movie:key:thematrix.1999");
    assert!(index[0]["install_b3"].as_str().is_some());
    assert!(index[0]["current_b3"].as_str().is_some());

    let dest = scratch("nm-dest");
    let dest_lib = dest.join("library");
    let import = bin()
        .args([
            "--json",
            "new-machine",
            "import",
            "--from",
            bundle.to_str().unwrap(),
            "--library-root",
            dest_lib.to_str().unwrap(),
            "--config-dir",
            dest.to_str().unwrap(),
            "--state-db",
            dest.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("import");
    assert_eq!(
        import.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&import.stderr)
    );
    let import_v = stdout_json(&import);
    assert_eq!(import_v["ok"], true);
    assert_eq!(import_v["data"]["titles"], 1);
    assert_eq!(
        std::fs::read(dest.join("desired-state.toml")).expect("ds"),
        DS_ZERO_FREE.as_bytes()
    );
    assert_eq!(
        std::fs::read(dest.join("tls/ca.pem")).expect("pem"),
        b"ca-bytes"
    );
    assert_eq!(
        std::fs::read(dest.join("tls/client.key")).expect("key"),
        b"key-bytes"
    );
    let canon = std::fs::canonicalize(&dest_lib).expect("canon");
    assert_eq!(
        import_v["data"]["library_root"].as_str().expect("root"),
        canon.to_str().expect("utf8")
    );
    for name in ["movies", "series", "music", "_ops", "_incoming"] {
        assert!(dest_lib.join(name).is_dir(), "{name}");
    }
    assert!(
        !dest_lib.join(rel).exists(),
        "layout bootstraps with no media"
    );

    let again = bin()
        .args([
            "--json",
            "new-machine",
            "import",
            "--from",
            bundle.to_str().unwrap(),
            "--library-root",
            dest_lib.to_str().unwrap(),
            "--config-dir",
            dest.to_str().unwrap(),
            "--state-db",
            dest.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("import again");
    assert_eq!(again.status.code(), Some(2), "non-empty title-index");
    let again_v = stdout_json(&again);
    assert_eq!(again_v["error"]["code"], "usage");
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&bundle);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn new_machine_import_git_work_tree_is_exit_5() {
    let bundle = scratch("nm-git-bundle");
    std::fs::write(bundle.join("desired-state.toml"), DS_ZERO_FREE).expect("ds");
    std::fs::write(bundle.join("title-index.json"), b"[]").expect("index");
    std::fs::create_dir_all(bundle.join("tls")).expect("tls");
    std::fs::write(bundle.join("tls/ca.pem"), b"ca").expect("pem");
    let dest = scratch("nm-git-dest");
    std::fs::create_dir_all(dest.join(".git")).expect("git");
    let output = bin()
        .args([
            "--json",
            "new-machine",
            "import",
            "--from",
            bundle.to_str().unwrap(),
            "--library-root",
            dest.join("library").to_str().unwrap(),
            "--config-dir",
            dest.to_str().unwrap(),
            "--state-db",
            dest.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("import git");
    assert_eq!(output.status.code(), Some(5));
    let value = stdout_json(&output);
    assert_eq!(value["error"]["code"], "policy_refusal");
    assert!(!dest.join("desired-state.toml").exists());
    assert!(!dest.join("tls/ca.pem").exists());
    let _ = std::fs::remove_dir_all(&bundle);
    let _ = std::fs::remove_dir_all(&dest);
}

#[test]
fn new_machine_import_tls_dir_git_work_tree_is_exit_5() {
    let bundle = scratch("nm-tls-git-bundle");
    std::fs::write(bundle.join("desired-state.toml"), DS_ZERO_FREE).expect("ds");
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
            "--json",
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
            "--state-db",
            dest.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("import tls git");
    assert_eq!(output.status.code(), Some(5));
    let value = stdout_json(&output);
    assert_eq!(value["error"]["code"], "policy_refusal");
    assert!(!tls_dir.join("ca.pem").exists());
    assert!(!dest.join("desired-state.toml").exists());
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
            "--json",
            "new-machine",
            "export",
            "--out",
            out.to_str().unwrap(),
            "--config-dir",
            src.to_str().unwrap(),
            "--state-db",
            src.join("state.db").to_str().unwrap(),
        ])
        .output()
        .expect("export git");
    assert_eq!(output.status.code(), Some(5));
    let value = stdout_json(&output);
    assert_eq!(value["error"]["code"], "policy_refusal");
    assert!(!out.join("desired-state.toml").exists());
    assert!(!out.join("tls/ca.pem").exists());
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn library_reindex_after_loss_then_clash_is_error() {
    let dir = scratch("reindex-cli");
    let lib = dir.join("library");
    let rel = "movies/The.Matrix.(1999)/The.Matrix.(1999).mkv";
    std::fs::create_dir_all(lib.join("movies/The.Matrix.(1999)")).expect("mkdir");
    std::fs::write(lib.join(rel), b"orig").expect("media");
    let db = dir.join("state.db");
    let first = bin()
        .args([
            "--json",
            "library",
            "reindex",
            "--library-root",
            lib.to_str().unwrap(),
            "--state-db",
            db.to_str().unwrap(),
        ])
        .output()
        .expect("reindex");
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(stdout_json(&first)["data"]["indexed"], 1);
    std::fs::write(lib.join(rel), b"changed").expect("change");
    let clash = bin()
        .args([
            "--json",
            "library",
            "reindex",
            "--library-root",
            lib.to_str().unwrap(),
            "--state-db",
            db.to_str().unwrap(),
        ])
        .output()
        .expect("clash");
    assert_eq!(
        clash.status.code(),
        Some(1),
        "digest clash must be runtime: {}",
        String::from_utf8_lossy(&clash.stderr)
    );
    let value = stdout_json(&clash);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "runtime");
    let message = value["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("immutable"), "{message}");
    let _ = std::fs::remove_dir_all(&dir);
}
