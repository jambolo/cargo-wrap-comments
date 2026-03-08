use std::fs;
use std::path::PathBuf;
use std::process::Command;

use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn cargo_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cargo-wrap-comments"))
}

fn temp_dir() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("cargo-wrap-comments-rustfmt-tests-{id}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn create_file(dir: &PathBuf, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, content).unwrap();
    path
}

const SHORT: &str = "// short\n";

/// .rustfmt.toml in the current directory is found.
#[test]
fn discovers_rustfmt_toml_in_current_dir() {
    let dir = temp_dir();
    let src = create_file(&dir, "test.rs", SHORT);
    create_file(&dir, ".rustfmt.toml", "max_width = 77\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&src)
        .current_dir(&dir)
        .env("HOME", &dir)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 77"), "stderr: {stderr}");
    assert!(stderr.contains(".rustfmt.toml"), "stderr: {stderr}");
}

/// rustfmt.toml (no dot prefix) is also found.
#[test]
fn discovers_rustfmt_toml_without_dot_prefix() {
    let dir = temp_dir();
    let src = create_file(&dir, "test.rs", SHORT);
    create_file(&dir, "rustfmt.toml", "max_width = 88\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&src)
        .current_dir(&dir)
        .env("HOME", &dir)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 88"), "stderr: {stderr}");
    assert!(stderr.contains("rustfmt.toml"), "stderr: {stderr}");
}

/// .rustfmt.toml takes precedence over rustfmt.toml in the same directory.
#[test]
fn dot_prefix_takes_precedence() {
    let dir = temp_dir();
    let src = create_file(&dir, "test.rs", SHORT);
    create_file(&dir, ".rustfmt.toml", "max_width = 71\n");
    create_file(&dir, "rustfmt.toml", "max_width = 72\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&src)
        .current_dir(&dir)
        .env("HOME", &dir)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 71"), "stderr: {stderr}");
}

/// Searches parent directories to find config.
#[test]
fn discovers_config_in_parent_directory() {
    let dir = temp_dir();
    let subdir = dir.join("sub");
    fs::create_dir_all(&subdir).unwrap();
    let src = create_file(&subdir, "test.rs", SHORT);
    create_file(&dir, ".rustfmt.toml", "max_width = 65\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&src)
        .current_dir(&subdir)
        .env("HOME", &dir)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 65"), "stderr: {stderr}");
}

/// Config in a closer directory wins over one further up.
#[test]
fn closer_config_wins() {
    let dir = temp_dir();
    let subdir = dir.join("sub");
    fs::create_dir_all(&subdir).unwrap();
    let src = create_file(&subdir, "test.rs", SHORT);
    create_file(&dir, ".rustfmt.toml", "max_width = 60\n");
    create_file(&subdir, ".rustfmt.toml", "max_width = 55\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&src)
        .current_dir(&subdir)
        .env("HOME", &dir)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 55"), "stderr: {stderr}");
}

/// Does not search beyond HOME.
#[test]
fn stops_at_home() {
    let dir = temp_dir();
    let home = dir.join("home");
    let subdir = home.join("project");
    fs::create_dir_all(&subdir).unwrap();
    let src = create_file(&subdir, "test.rs", SHORT);
    // Config above HOME should be ignored
    create_file(&dir, ".rustfmt.toml", "max_width = 42\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&src)
        .current_dir(&subdir)
        .env("HOME", &home)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 100 (default)"), "stderr: {stderr}");
}

/// Config file without max_width falls back to default.
#[test]
fn config_without_max_width_uses_default() {
    let dir = temp_dir();
    let src = create_file(&dir, "test.rs", SHORT);
    create_file(&dir, ".rustfmt.toml", "edition = \"2021\"\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&src)
        .current_dir(&dir)
        .env("HOME", &dir)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 100 (default)"), "stderr: {stderr}");
}

/// CLI --max-width overrides rustfmt.toml.
#[test]
fn cli_overrides_config_file() {
    let dir = temp_dir();
    let src = create_file(&dir, "test.rs", SHORT);
    create_file(&dir, ".rustfmt.toml", "max_width = 77\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v", "-w", "90"])
        .arg(&src)
        .current_dir(&dir)
        .env("HOME", &dir)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 90 (from --max-width)"), "stderr: {stderr}");
}
