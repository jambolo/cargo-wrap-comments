use std::fs;
use std::process::Command;

fn cargo_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cargo-wrap-comments"))
}

fn create_temp_file(name: &str, content: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("cargo-wrap-comments-verbose-tests");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, content).unwrap();
    path
}

const WRAPPABLE: &str = "// This is a very long comment line that will definitely exceed the default width of one hundred characters and need wrapping\n";
const SHORT: &str = "// short\n";

// --- Width source ---

#[test]
fn verbose_shows_width_from_cli_arg() {
    let path = create_temp_file("v_width_cli.rs", SHORT);
    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v", "-w", "80"])
        .arg(&path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Width: 80 (from --max-width)"),
        "stderr: {stderr}"
    );
}

#[test]
fn verbose_shows_default_width() {
    let path = create_temp_file("v_width_default.rs", SHORT);
    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 100 (default)"), "stderr: {stderr}");
}

// --- File discovery ---

#[test]
fn verbose_shows_file_count() {
    let path = create_temp_file("v_count.rs", SHORT);
    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Found 1 file(s) to process."),
        "stderr: {stderr}"
    );
}

// --- Per-file status ---

#[test]
fn verbose_unchanged_file_single_line() {
    let path = create_temp_file("v_unchanged.rs", SHORT);
    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr
        .lines()
        .find(|l| l.contains("unchanged"))
        .expect(&format!("no unchanged line in stderr: {stderr}"));
    assert!(line.contains("1 lines"), "missing line count: {line}");
    assert!(line.contains("v_unchanged.rs"), "missing file path: {line}");
}

#[test]
fn verbose_modified_file_single_line() {
    let path = create_temp_file("v_modified.rs", WRAPPABLE);
    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr
        .lines()
        .find(|l| l.contains("change(s)"))
        .expect(&format!("no change(s) line in stderr: {stderr}"));
    assert!(line.contains("1 lines"), "missing line count: {line}");
    assert!(line.contains("1 change(s)"), "wrong change count: {line}");
}

// --- Stdout vs stderr ---

#[test]
fn check_diff_output_on_stdout() {
    let path = create_temp_file("v_stdout.rs", WRAPPABLE);
    let output = cargo_bin()
        .args(["wrap-comments", "--check"])
        .arg(&path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Would modify:"),
        "check output should be on stdout: {stdout}"
    );
}

#[test]
fn modified_message_on_stderr() {
    let path = create_temp_file("v_stderr.rs", WRAPPABLE);
    let output = cargo_bin()
        .args(["wrap-comments"])
        .arg(&path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.is_empty(), "stdout should be empty: {stdout}");
    assert!(
        stderr.contains("Modified:"),
        "Modified should be on stderr: {stderr}"
    );
}

#[test]
fn summary_on_stderr() {
    let path = create_temp_file("v_summary.rs", SHORT);
    let output = cargo_bin()
        .args(["wrap-comments", "--check"])
        .arg(&path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Processed"),
        "summary should be on stderr: {stderr}"
    );
}

// --- Quiet ---

#[test]
fn quiet_suppresses_summary() {
    let path = create_temp_file("v_quiet.rs", SHORT);
    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-q"])
        .arg(&path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Processed"),
        "quiet should suppress summary: {stderr}"
    );
}

// --- Verbose overrides quiet ---

#[test]
fn verbose_overrides_quiet() {
    let path = create_temp_file("v_override.rs", SHORT);
    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v", "-q"])
        .arg(&path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Processed"),
        "verbose should override quiet for summary: {stderr}"
    );
    assert!(
        stderr.contains("Width:"),
        "verbose should show width: {stderr}"
    );
    assert!(
        stderr.contains("Found"),
        "verbose should show file count: {stderr}"
    );
}

// --- No verbose by default ---

#[test]
fn no_verbose_hides_details() {
    let path = create_temp_file("v_noverbose.rs", SHORT);
    let output = cargo_bin()
        .args(["wrap-comments", "--check"])
        .arg(&path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Width:"),
        "should not show width without -v: {stderr}"
    );
    assert!(
        !stderr.contains("Found"),
        "should not show file count without -v: {stderr}"
    );
    assert!(
        !stderr.contains("unchanged"),
        "should not show per-file status without -v: {stderr}"
    );
}
