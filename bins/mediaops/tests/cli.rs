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
        serde_json::from_str::<Value>(stdout.trim()).is_err(),
        "help must not be a JSON envelope: {stdout}"
    );
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
