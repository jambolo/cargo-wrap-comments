use std::io::Write;
use std::process::{Command, Stdio};

fn cargo_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cargo-wrap-comments"))
}

fn run_stdin(input: &str, args: &[&str]) -> std::process::Output {
    let mut child = cargo_bin()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

// --- Basic stdin/stdout ---

#[test]
fn stdin_passes_through_unchanged_content() {
    let input = "// short\nfn main() {}\n";
    let output = run_stdin(input, &["wrap-comments"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), input);
}

#[test]
fn stdin_wraps_long_comment() {
    let input = "// This is a long comment that should definitely be wrapped when the max width is set to forty characters.\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "40"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        assert!(
            line.len() <= 40,
            "line exceeds width 40: {line:?} (len={})",
            line.len()
        );
    }
}

#[test]
fn stdin_combines_consecutive_comments() {
    let input = "// hello\n// world\n";
    let output = run_stdin(input, &["wrap-comments"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "// hello world\n");
}

#[test]
fn stdin_preserves_non_comments() {
    let input = "let x = 5;\nlet y = 10;\n";
    let output = run_stdin(input, &["wrap-comments"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), input);
}

#[test]
fn stdin_preserves_blank_comment_separator() {
    let input = "// line one\n//\n// line two\n";
    let output = run_stdin(input, &["wrap-comments"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), input);
}

#[test]
fn stdin_preserves_code_blocks() {
    let input = "// ```\n// this is a very long line inside a code block that should not be wrapped even though it exceeds the width limit\n// ```\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "40"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), input);
}

#[test]
fn stdin_does_not_combine_inside_code_blocks() {
    let input = "// ```\n// line one\n// line two\n// ```\n";
    let output = run_stdin(input, &["wrap-comments"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), input);
}

#[test]
fn stdin_resumes_combining_after_code_block() {
    let input = "// ```\n// code\n// ```\n// hello\n// world\n";
    let output = run_stdin(input, &["wrap-comments"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "// ```\n// code\n// ```\n// hello world\n"
    );
}

#[test]
fn stdin_preserves_different_indentation() {
    let input = "    // indented\n// not indented\n";
    let output = run_stdin(input, &["wrap-comments"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), input);
}

#[test]
fn stdin_does_not_combine_different_markers() {
    let input = "// regular\n/// doc\n";
    let output = run_stdin(input, &["wrap-comments"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), input);
}

#[test]
fn stdin_does_not_combine_before_hierarchical_marker() {
    let input = "// first line\n// - bullet\n";
    let output = run_stdin(input, &["wrap-comments"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), input);
}

#[test]
fn stdin_wraps_bullet_with_continuation_indent() {
    let input = "// - This is a very long bullet point that should wrap with proper indentation\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "40"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "// - This is a very long bullet point\n//   that should wrap with proper\n//   indentation\n"
    );
}

#[test]
fn stdin_wraps_numbered_with_continuation_indent() {
    let input =
        "// 1. This is a very long numbered item that should wrap with proper indentation\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "40"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "// 1. This is a very long numbered item\n//    that should wrap with proper\n//    indentation\n"
    );
}

#[test]
fn stdin_wraps_multi_digit_numbered_with_continuation_indent() {
    let input =
        "// 10. This is a very long numbered item that should wrap with proper indentation\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "40"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "// 10. This is a very long numbered item\n//     that should wrap with proper\n//     indentation\n"
    );
}

#[test]
fn stdin_no_trailing_spaces() {
    let input =
        "//\n// This is a long comment that needs to be wrapped because it exceeds the width\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "40"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        assert_eq!(line, line.trim_end(), "trailing spaces in: {line:?}");
    }
}

#[test]
fn stdin_preserves_backtick_spans() {
    let input = "// This uses `long code span` and should not break inside backticks\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "35"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let text = line.trim_start_matches("//").trim();
        let backtick_count = text.chars().filter(|&c| c == '`').count();
        assert_eq!(backtick_count % 2, 0, "unmatched backtick in: {line:?}");
    }
}

#[test]
fn stdin_handles_doc_comments() {
    let input =
        "/// This is a very long doc comment that should be wrapped at a reasonable width.\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "40"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        assert!(
            line.starts_with("///"),
            "expected doc comment marker: {line:?}"
        );
    }
}

#[test]
fn stdin_handles_bang_comments() {
    let input = "//! This is a very long module doc comment that should be wrapped at a reasonable width.\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "40"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        assert!(
            line.starts_with("//!"),
            "expected bang comment marker: {line:?}"
        );
    }
}

#[test]
fn stdin_handles_mixed_code_and_comments() {
    let input =
        "fn foo() {\n    // This is a comment that is long enough to wrap\n    let x = 42;\n}\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "30"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fn foo()"));
    assert!(stdout.contains("let x = 42;"));
}

#[test]
fn stdin_handles_empty_input() {
    let output = run_stdin("", &["wrap-comments"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

#[test]
fn stdin_handles_no_trailing_newline() {
    let input = "// hello\n// world";
    let output = run_stdin(input, &["wrap-comments"]);
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "// hello world");
}

// --- Check mode with stdin ---

#[test]
fn stdin_check_no_changes_exits_success() {
    let input = "// short\n";
    let output = run_stdin(input, &["wrap-comments", "--check"]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn stdin_check_with_changes_exits_failure() {
    let input = "// This is a long comment that should definitely be wrapped when the max width is set to forty characters.\n";
    let output = run_stdin(input, &["wrap-comments", "--check", "-w", "40"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Line"), "expected diff output: {stdout}");
}

#[test]
fn stdin_check_shows_diff() {
    let input = "// hello\n// world\n";
    let output = run_stdin(input, &["wrap-comments", "--check"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("- // hello"),
        "expected removed line: {stdout}"
    );
    assert!(
        stdout.contains("- // world"),
        "expected removed line: {stdout}"
    );
    assert!(
        stdout.contains("+ // hello world"),
        "expected added line: {stdout}"
    );
}

// --- Width flag with stdin ---

#[test]
fn stdin_respects_max_width_flag() {
    let input = "// aaaa bbbb cccc dddd eeee ffff\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "20"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        assert!(
            line.len() <= 20,
            "line exceeds width 20: {line:?} (len={})",
            line.len()
        );
    }
}

// --- No summary output in stdin mode ---

#[test]
fn stdin_no_summary_on_stderr() {
    let input = "// hello\n// world\n";
    let output = run_stdin(input, &["wrap-comments"]);
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Processed"),
        "stdin mode should not print summary: {stderr}"
    );
}

// --- Heading wrapping ---

#[test]
fn stdin_wraps_heading_with_continuation_indent() {
    let input = "// ## This is a very long heading that should wrap with proper indentation\n";
    let output = run_stdin(input, &["wrap-comments", "-w", "40"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(lines.len() > 1, "heading should have wrapped");
    for line in &lines[1..] {
        assert!(
            line.starts_with("//    "),
            "continuation should have heading indent: {line:?}"
        );
    }
}
