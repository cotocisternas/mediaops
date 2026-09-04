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
        "plan", "run", "watch", "why", "status", "encode", "hold", "reclaim", "doctor",
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
        service.contains("--state-db"),
        "ExecStart must pin --state-db, got {service}"
    );
    assert!(service.contains("TimeoutStartSec=infinity"));
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
