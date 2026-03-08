use std::fs;
use std::process::Command;

fn cargo_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cargo-wrap-comments"))
}

fn create_temp_file(name: &str, content: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("cargo-wrap-comments-tests");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    path
}

// A comment long enough to be wrapped at width 40.
const WRAPPABLE_CONTENT: &str = "// This is a long comment that should definitely be wrapped when the max width is set to forty characters.\n";

// Content that needs no wrapping.
const UNCHANGED_CONTENT: &str = "// short\nfn main() {}\n";

#[test]
fn quiet_long_flag_suppresses_output_when_file_modified() {
    let path = create_temp_file("quiet_long.rs", WRAPPABLE_CONTENT);
    let output = cargo_bin()
        .args([
            "wrap-comments",
            "--quiet",
            "-w",
            "40",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "expected no stdout with --quiet, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "expected no stderr with --quiet, got: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn quiet_short_flag_suppresses_output_when_file_modified() {
    let path = create_temp_file("quiet_short.rs", WRAPPABLE_CONTENT);
    let output = cargo_bin()
        .args(["wrap-comments", "-q", "-w", "40", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "expected no stdout with -q, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "expected no stderr with -q, got: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn quiet_suppresses_output_when_no_changes() {
    let path = create_temp_file("quiet_nochange.rs", UNCHANGED_CONTENT);
    let output = cargo_bin()
        .args(["wrap-comments", "--quiet", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "expected no stdout with --quiet when no changes, got: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "expected no stderr with --quiet when no changes, got: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn quiet_with_check_still_produces_output() {
    let path = create_temp_file("quiet_check.rs", WRAPPABLE_CONTENT);
    let output = cargo_bin()
        .args([
            "wrap-comments",
            "--quiet",
            "--check",
            "-w",
            "40",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !output.stdout.is_empty(),
        "expected --check to override --quiet and produce stdout"
    );
}

#[test]
fn quiet_short_with_check_still_produces_output() {
    let path = create_temp_file("quiet_short_check.rs", WRAPPABLE_CONTENT);
    let output = cargo_bin()
        .args([
            "wrap-comments",
            "-q",
            "--check",
            "-w",
            "40",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !output.stdout.is_empty(),
        "expected --check to override -q and produce stdout"
    );
}

#[test]
fn without_quiet_produces_output() {
    let path = create_temp_file("no_quiet.rs", WRAPPABLE_CONTENT);
    let output = cargo_bin()
        .args(["wrap-comments", "-w", "40", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!output.stderr.is_empty(), "expected stderr without --quiet");
}
