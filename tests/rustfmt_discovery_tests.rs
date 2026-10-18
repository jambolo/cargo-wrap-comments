use std::fs;
use std::path::{Path, PathBuf};
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

fn create_file(dir: &Path, name: &str, content: &str) -> PathBuf {
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

/// Like rustfmt, the search continues above HOME up to the filesystem root.
#[test]
fn searches_above_home() {
    let dir = temp_dir();
    let home = dir.join("home");
    let subdir = home.join("project");
    fs::create_dir_all(&subdir).unwrap();
    let src = create_file(&subdir, "test.rs", SHORT);
    create_file(&dir, ".rustfmt.toml", "max_width = 42\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&src)
        .current_dir(&subdir)
        .env("HOME", &home)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 42"), "stderr: {stderr}");
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

const WRAPPABLE: &str = "// one two three four five six seven eight nine ten eleven twelve\n";

/// The search starts in the file's directory, not the current directory.
#[test]
fn resolves_from_file_directory_not_current_directory() {
    let dir = temp_dir();
    let src = create_file(&dir.join("a"), "test.rs", SHORT);
    create_file(&dir.join("a"), ".rustfmt.toml", "max_width = 66\n");
    create_file(&dir.join("b"), ".rustfmt.toml", "max_width = 99\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&src)
        .current_dir(dir.join("b"))
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 66"), "stderr: {stderr}");
    assert!(!stderr.contains("Width: 99"), "stderr: {stderr}");
}

/// Files in different directories each use their own config.
#[test]
fn each_file_uses_its_own_config() {
    let dir = temp_dir();
    let a = create_file(&dir.join("a"), "a.rs", SHORT);
    let b = create_file(&dir.join("b"), "b.rs", SHORT);
    create_file(&dir.join("a"), ".rustfmt.toml", "max_width = 51\n");
    create_file(&dir.join("b"), ".rustfmt.toml", "max_width = 52\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&a)
        .arg(&b)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let width_for = |name: &str| {
        stderr
            .lines()
            .find(|l| l.starts_with("Width:") && l.ends_with(name))
            .unwrap_or_default()
            .to_string()
    };
    assert!(width_for("a.rs").starts_with("Width: 51"), "stderr: {stderr}");
    assert!(width_for("b.rs").starts_with("Width: 52"), "stderr: {stderr}");
}

/// Stdin resolves the config from the current directory.
#[test]
fn stdin_resolves_from_current_directory() {
    use std::io::Write;
    use std::process::Stdio;

    let dir = temp_dir();
    create_file(&dir, ".rustfmt.toml", "max_width = 30\n");

    let mut child = cargo_bin()
        .args(["wrap-comments", "-v"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(WRAPPABLE.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 30"), "stderr: {stderr}");
    assert!(String::from_utf8_lossy(&output.stdout).lines().all(|l| l.len() <= 30));
}

/// --config-path with a file uses that file, whatever its name, instead of searching.
#[test]
fn config_path_file() {
    let dir = temp_dir();
    let src = create_file(&dir, "test.rs", SHORT);
    create_file(&dir, ".rustfmt.toml", "max_width = 74\n");
    let custom = create_file(&dir.join("cfg"), "custom.toml", "max_width = 73\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v", "--config-path"])
        .arg(&custom)
        .arg(&src)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 73"), "stderr: {stderr}");
    assert!(stderr.contains("custom.toml"), "stderr: {stderr}");
}

/// --config-path with a directory uses the config file directly inside it.
#[test]
fn config_path_directory() {
    let dir = temp_dir();
    let src = create_file(&dir, "test.rs", SHORT);
    create_file(&dir.join("cfg"), "rustfmt.toml", "max_width = 75\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v", "--config-path"])
        .arg(dir.join("cfg"))
        .arg(&src)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 75"), "stderr: {stderr}");
}

/// --config-path with a directory does not search its ancestors.
#[test]
fn config_path_directory_without_config_is_error() {
    let dir = temp_dir();
    let src = create_file(&dir, "test.rs", SHORT);
    create_file(&dir, ".rustfmt.toml", "max_width = 76\n");
    fs::create_dir_all(dir.join("empty")).unwrap();

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "--config-path"])
        .arg(dir.join("empty"))
        .arg(&src)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unable to find a config file"), "stderr: {stderr}");
}

/// --config-path with a nonexistent path is an error.
#[test]
fn config_path_missing_is_error() {
    let dir = temp_dir();
    let src = create_file(&dir, "test.rs", SHORT);

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "--config-path"])
        .arg(dir.join("missing.toml"))
        .arg(&src)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unable to find a config file"), "stderr: {stderr}");
}

/// An unparsable config file is an error, and no file is modified.
#[test]
fn invalid_toml_is_error() {
    let dir = temp_dir();
    let src = create_file(&dir, "test.rs", WRAPPABLE);
    create_file(&dir, ".rustfmt.toml", "max_width = \n");

    let output = cargo_bin().args(["wrap-comments", "-w", "30"]).arg(&src).output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("could not parse config file"), "stderr: {stderr}");
    assert_eq!(fs::read_to_string(&src).unwrap(), WRAPPABLE);
}

/// A `max_width` that is not a non-negative integer is an error.
#[test]
fn invalid_max_width_is_error() {
    let dir = temp_dir();
    let src = create_file(&dir, "test.rs", SHORT);
    create_file(&dir, ".rustfmt.toml", "max_width = \"wide\"\n");

    let output = cargo_bin().args(["wrap-comments", "--check"]).arg(&src).output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid `max_width`"), "stderr: {stderr}");
}

/// With no config above the file, the home directory is checked.
#[cfg(unix)]
#[test]
fn falls_back_to_home_directory() {
    let dir = temp_dir();
    let src = create_file(&dir.join("project"), "test.rs", SHORT);
    create_file(&dir.join("home"), ".rustfmt.toml", "max_width = 61\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&src)
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("xdg"))
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 61"), "stderr: {stderr}");
}

/// With no config above the file or in the home directory, `<user config dir>/rustfmt` is checked.
#[cfg(target_os = "linux")]
#[test]
fn falls_back_to_user_config_directory() {
    let dir = temp_dir();
    let src = create_file(&dir.join("project"), "test.rs", SHORT);
    fs::create_dir_all(dir.join("home")).unwrap();
    create_file(&dir.join("xdg").join("rustfmt"), "rustfmt.toml", "max_width = 62\n");

    let output = cargo_bin()
        .args(["wrap-comments", "--check", "-v"])
        .arg(&src)
        .env("HOME", dir.join("home"))
        .env("XDG_CONFIG_HOME", dir.join("xdg"))
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Width: 62"), "stderr: {stderr}");
}
