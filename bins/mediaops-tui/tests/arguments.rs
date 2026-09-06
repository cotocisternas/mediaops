use std::process::{Command, Stdio};

#[test]
fn help_and_version_work_without_tty_or_api() {
    for flag in ["--help", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_mediaops-tui"))
            .arg(flag)
            .stdin(Stdio::null())
            .env_remove("TERM")
            .output()
            .expect("run");
        assert!(output.status.success());
        assert!(!output.stdout.is_empty());
        assert!(!output.stdout.contains(&27));
        assert!(!output.stderr.contains(&27));
    }
}

#[test]
fn interactive_mode_refuses_pipes_before_terminal_initialization() {
    let output = Command::new(env!("CARGO_BIN_EXE_mediaops-tui"))
        .stdin(Stdio::null())
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.contains(&27));
}

#[test]
fn json_and_unknown_flags_are_tui_usage_errors() {
    for flag in ["--json", "--unknown"] {
        let output = Command::new(env!("CARGO_BIN_EXE_mediaops-tui"))
            .arg(flag)
            .stdin(Stdio::null())
            .output()
            .expect("run");
        assert_eq!(output.status.code(), Some(2));
        assert!(!output.stdout.contains(&27));
        assert!(!output.stderr.contains(&27));
    }
}
