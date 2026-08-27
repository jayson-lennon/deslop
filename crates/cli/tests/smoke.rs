//! Black-box smoke tests: the binary contract (help + exit codes).

use assert_cmd::Command;

fn deslop() -> Command {
    Command::cargo_bin("deslop").expect("binary builds")
}

#[test]
fn help_prints_usage_and_exits_zero() {
    // Given the built CLI.

    // When asking for help.
    let output = deslop().arg("--help").output().expect("runs");

    // Then usage text is present and exit is 0.
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"), "expected Usage in help");
    for subcommand in ["fix", "rules", "init"] {
        assert!(
            stdout.contains(subcommand),
            "expected `{subcommand}` listed in help"
        );
    }
}

#[test]
fn missing_path_exits_two_with_message() {
    // Given a path that does not exist.

    // When linting it.
    let output = deslop()
        .arg("/definitely/not/here.md")
        .output()
        .expect("runs");

    // Then exit is 2 and the message names the path.
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("does not exist"));
}

#[test]
fn explicit_config_missing_exits_two() {
    // Given --config pointing nowhere.
    let cfg = "/definitely/not/.deslop.toml";

    // When invoking.
    let output = deslop()
        .arg("--config")
        .arg(cfg)
        .arg(".")
        .output()
        .expect("runs");

    // Then exit is 2 naming the config file.
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--config"));
}
