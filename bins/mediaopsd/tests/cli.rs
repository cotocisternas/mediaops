use std::process::{Command, Output};

use serde_json::Value;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mediaopsd"))
}

fn stdout_json(output: &Output) -> Value {
    assert!(
        !output.stdout.contains(&0x1b),
        "JSON cannot contain terminal escapes"
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("one raw JSON object");
    assert!(
        value.get("ok").is_none(),
        "retired envelope must not return"
    );
    assert!(
        value.get("data").is_none(),
        "retired envelope must not return"
    );
    value
}

#[test]
fn json_identity_is_raw_and_quiet() {
    let output = bin().args(["-o", "json"]).output().expect("identity");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        stdout_json(&output),
        serde_json::json!({
            "name": "mediaopsd", "version": env!("CARGO_PKG_VERSION")
        })
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn human_identity_is_quiet() {
    let output = bin().output().expect("identity");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("mediaopsd {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn human_usage_error_is_printed_once() {
    let output = bin().arg("--not-a-flag").output().expect("usage");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--not-a-flag"));
    assert_eq!(stderr.matches("error:").count(), 1, "{stderr}");
    assert!(!stderr.contains("command failed"));
    assert!(!output.stderr.contains(&0x1b));
}

#[test]
fn structured_parse_errors_recognize_output_before_or_after_bad_arguments() {
    for args in [
        vec!["-o", "json", "--not-a-flag"],
        vec!["--not-a-flag", "-o", "json"],
        vec!["--not-a-flag", "--output=json"],
        vec!["--not-a-flag", "-ojson"],
        vec!["--not-a-flag", "-o=json"],
    ] {
        let output = bin().args(&args).output().expect("usage");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let value = stdout_json(&output);
        assert_eq!(value["error"]["code"], "usage");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("--not-a-flag")
        );
        assert!(output.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn retired_flags_and_unknown_output_are_rejected() {
    for args in [
        vec!["--json"],
        vec!["--json=true"],
        vec!["-o", "xml"],
        vec!["serve", "--socket", "/tmp/old.sock", "--tls-dir", "/tmp"],
        vec!["serve", "--upstream", "old-box", "--tls-dir", "/tmp"],
    ] {
        let output = bin().args(&args).output().expect("rejected flag");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn help_and_version_are_quiet_without_needing_a_server() {
    for args in [vec!["--help"], vec!["-o", "json", "--help"]] {
        let output = bin().args(args).output().expect("help");
        assert_eq!(output.status.code(), Some(0));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Usage:"));
        assert!(stdout.contains("--output"));
        assert!(!stdout.contains("--json"));
        assert!(output.stderr.is_empty());
    }
    let output = bin().arg("--version").output().expect("version");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("mediaopsd {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn serve_help_describes_only_current_seedbox_arguments() {
    let output = bin()
        .args(["serve", "--help"])
        .output()
        .expect("serve help");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("0.0.0.0:50051"));
    assert!(stdout.contains("--root"));
    assert!(!stdout.contains("--socket"));
    assert!(!stdout.contains("--upstream"));
    assert!(output.stderr.is_empty());
}

#[test]
fn unused_roles_are_usage_errors_without_starting_a_server() {
    for role in ["home", "reverse-connect"] {
        let output = bin()
            .args(["serve", "--role", role, "--tls-dir", "/tmp"])
            .output()
            .expect("unused role");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.starts_with("error: "), "{stderr}");
        assert!(stderr.contains(role), "{stderr}");
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
    }
}

#[test]
fn runtime_and_role_errors_use_the_same_raw_error_shape() {
    let output = bin()
        .args(["serve", "--role", "home", "--tls-dir", "/tmp", "-o", "json"])
        .output()
        .expect("role error");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout_json(&output)["error"]["code"], "usage");
    assert!(output.stderr.is_empty());

    let absent = std::env::temp_dir().join(format!(
        "mediaopsd-no-tls-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = bin()
        .args(["serve", "--root", "media=/tmp", "--tls-dir"])
        .arg(absent)
        .args(["-o", "json"])
        .output()
        .expect("TLS error");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout_json(&output)["error"]["code"], "runtime");
    assert!(output.stderr.is_empty());
}
