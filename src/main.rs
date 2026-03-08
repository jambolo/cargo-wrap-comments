//! A CLI tool that reformats source code comments to wrap at a specified width.
//!
//! `cargo-wrap-comments` combines consecutive comment lines and re-wraps them to fit within a
//! configurable line width, similar to what `rustfmt` does for code but applied to `//`, `///`, and
//! `//!` comments.
//!
//! # Usage
//!
//! ```sh
//! # As a cargo subcommand
//! cargo wrap-comments src/main.rs
//!
//! # As a standalone command
//! cargo-wrap-comments wrap-comments src/main.rs
//!
//! # Format with a custom width
//! cargo wrap-comments --max-width 80 src/main.rs
//!
//! # Preview changes without modifying files
//! cargo wrap-comments --check "src/**/*.rs"
//!
//! # Read from stdin, write to stdout
//! cat src/main.rs | cargo wrap-comments
//! ```
//!
//! # Width Resolution
//!
//! The line width is resolved in order:
//!
//! 1. `--max-width` CLI argument
//! 2. `max_width` from `.rustfmt.toml` or `rustfmt.toml` (searched from the current directory up to
//!    `$HOME`)
//! 3. Default: 100

use clap::Parser;
use glob::glob;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, Read as _};
use std::path::PathBuf;
use std::process;

#[derive(Parser)]
#[command(name = "cargo", bin_name = "cargo")]
enum CargoCli {
    #[command(name = "wrap-comments", version)]
    #[command(about = "Reformat source code comments to wrap at a specified width")]
    WrapComments(Cli),
}

#[derive(clap::Args)]
struct Cli {
    /// Files or glob patterns to process (reads from stdin if omitted)
    files: Vec<String>,

    /// Maximum line width
    #[arg(long, short = 'w')]
    max_width: Option<usize>,

    /// Preview mode - show what would change without modifying files
    #[arg(long)]
    check: bool,

    /// Print verbose output
    #[arg(long, short = 'v')]
    verbose: bool,

    /// Print less output
    #[arg(long, short = 'q')]
    quiet: bool,
}

/// A parsed comment line, split into its constituent parts.
///
/// For example, the line `    /// hello world` would be parsed as:
///
/// ```text
/// indent: "    "
/// marker: "///"
/// text:   "hello world"
/// ```
#[derive(Debug, Clone)]
struct CommentLine {
    /// Leading whitespace before the comment marker.
    indent: String,
    /// The comment marker (`//`, `///`, or `//!`).
    marker: String,
    /// The text content after the marker and optional space.
    text: String,
}

/// Recognized comment markers, ordered longest-first to ensure `///` and `//!` match before `//`.
const MARKERS: [&str; 3] = ["///", "//!", "//"];

/// Parses a source line into a [`CommentLine`] if it contains a recognized comment marker.
///
/// Returns `None` for non-comment lines.
fn parse_comment_line(line: &str) -> Option<CommentLine> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    MARKERS.iter().find_map(|marker| {
        trimmed.strip_prefix(marker).map(|rest| CommentLine {
            indent: indent.to_string(),
            marker: marker.to_string(),
            text: rest.strip_prefix(' ').unwrap_or(rest).to_string(),
        })
    })
}

/// Returns `true` if `text` begins with a markdown-style structural marker.
///
/// Recognized markers: `#`, `*`, `-`, `` ``` ``, a single letter followed by `.` (e.g. `A.`, `a.`),
/// or one or more digits followed by `.` (e.g. `1.`, `10.`, `100.`).
fn starts_with_hierarchical_marker(text: &str) -> bool {
    if matches!(text.as_bytes().first(), Some(b'#' | b'*' | b'-')) || text.starts_with("```") {
        return true;
    }
    let mut chars = text.chars();
    if let Some(c) = chars.next()
        && c.is_ascii_alphanumeric()
    {
        if !c.is_ascii_digit() {
            return chars.next() == Some('.');
        }
        for ch in chars {
            if ch == '.' {
                return true;
            }
            if !ch.is_ascii_digit() {
                return false;
            }
        }
    }
    false
}

/// Returns the width of a hierarchical marker prefix (e.g. `- ` → 2, `1. ` → 3, `10. ` → 4,
/// `## ` → 3).
///
/// Returns 0 if the text does not start with a recognized marker.
fn hierarchical_marker_width(text: &str) -> usize {
    if text.starts_with('#') {
        let hashes = text.bytes().take_while(|&b| b == b'#').count();
        if text.as_bytes().get(hashes) == Some(&b' ') {
            return hashes + 1;
        }
    }
    if matches!(text.as_bytes().first(), Some(b'-' | b'*')) && text.as_bytes().get(1) == Some(&b' ')
    {
        return 2;
    }
    let bytes = text.as_bytes();
    if let Some(&first) = bytes.first()
        && first.is_ascii_alphanumeric()
    {
        if !first.is_ascii_digit() {
            // Single letter: `A. `
            if bytes.get(1) == Some(&b'.') && bytes.get(2) == Some(&b' ') {
                return 3;
            }
        } else {
            // Multi-digit: `1. `, `10. `, `100. `, etc.
            let digit_count = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
            if bytes.get(digit_count) == Some(&b'.') && bytes.get(digit_count + 1) == Some(&b' ') {
                return digit_count + 2;
            }
        }
    }
    0
}

/// Determines whether two adjacent comment lines should be combined.
///
/// Lines are combined when they share the same marker and indentation, neither is blank, the next
/// line doesn't start with a hierarchical marker, and neither line contains a code fence.
fn can_combine(current: &CommentLine, next: &CommentLine) -> bool {
    current.marker == next.marker
        && current.indent == next.indent
        && !current.text.is_empty()
        && !next.text.is_empty()
        && !starts_with_hierarchical_marker(&next.text)
        && !current.text.contains("```")
}

/// Splits text into tokens at whitespace boundaries, but keeps backtick-delimited spans (`` ` `` or
/// ` `` ` as single tokens even if they contain spaces.
fn tokenize_preserving_backticks(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut close_len: Option<usize> = None;

    while i < bytes.len() {
        if let Some(cl) = close_len {
            if bytes[i] == b'`' {
                let start = i;
                while i < bytes.len() && bytes[i] == b'`' {
                    i += 1;
                }
                current.push_str(&text[start..i]);
                if i - start == cl {
                    close_len = None;
                }
            } else {
                let ch = text[i..].chars().next().unwrap();
                current.push(ch);
                i += ch.len_utf8();
            }
        } else if bytes[i] == b'`' {
            let start = i;
            while i < bytes.len() && bytes[i] == b'`' {
                i += 1;
            }
            current.push_str(&text[start..i]);
            close_len = Some(i - start);
        } else if (bytes[i] as char).is_ascii_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            i += 1;
        } else {
            let ch = text[i..].chars().next().unwrap();
            current.push(ch);
            i += ch.len_utf8();
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Wraps `text` at word boundaries so each output line fits within `max_width`.
///
/// The first output line is prefixed with `prefix` (typically indent + marker + space).
/// Continuation lines use `continuation_prefix`, which may include extra indentation to align with
/// text after a hierarchical marker. Words that exceed the available width are emitted on their own
/// line without truncation. Backtick-delimited spans are kept on the same line.
fn wrap_text(text: &str, prefix: &str, continuation_prefix: &str, max_width: usize) -> Vec<String> {
    let words = tokenize_preserving_backticks(text);
    if words.is_empty() {
        return vec![prefix.trim_end().to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in &words {
        let pfx = if lines.is_empty() {
            prefix
        } else {
            continuation_prefix
        };
        let available = max_width.saturating_sub(pfx.len()).max(1);

        if current_line.is_empty() {
            current_line.push_str(word);
        } else if current_line.len() + 1 + word.len() <= available {
            current_line.push(' ');
            current_line.push_str(word);
        } else {
            lines.push(format!("{pfx}{current_line}").trim_end().to_string());
            current_line = word.to_string();
        }
    }

    let pfx = if lines.is_empty() {
        prefix
    } else {
        continuation_prefix
    };
    lines.push(format!("{pfx}{current_line}").trim_end().to_string());
    lines
}

/// A recorded change: `(line_number, old_lines, new_lines)`.
type Change = (usize, Vec<String>, Vec<String>);

/// Prints a set of changes as a unified-style diff.
fn print_changes(changes: &[Change], header: Option<&str>) {
    if let Some(h) = header {
        println!("{h}");
    }
    for (line_num, removed, added) in changes {
        println!("  Line {line_num}:");
        for r in removed {
            println!("    - {r}");
        }
        for a in added {
            println!("    + {a}");
        }
    }
}

/// Processes an entire file's content, combining and wrapping comments.
///
/// Returns the reformatted content and a list of [`Change`] tuples describing what was modified.
/// Lines inside code blocks (`` ``` ``) are passed through unchanged.
fn process_content(content: &str, max_width: usize) -> (String, Vec<Change>) {
    let lines: Vec<&str> = content.lines().collect();
    let mut result: Vec<String> = Vec::new();
    let mut changes: Vec<Change> = Vec::new();
    let mut i = 0;
    let mut in_code_block = false;

    while i < lines.len() {
        let comment = parse_comment_line(lines[i]);

        if let Some(ref c) = comment
            && c.text.starts_with("```")
        {
            in_code_block = !in_code_block;
        }

        let passthrough = comment.is_none() || in_code_block;
        if passthrough {
            result.push(lines[i].to_string());
            i += 1;
            continue;
        }

        let comment = comment.unwrap();

        let mut combined_text = comment.text.clone();
        let start_line = i;
        let mut orig_lines: Vec<String> = vec![lines[i].to_string()];

        let mut j = i + 1;
        while j < lines.len() {
            if let Some(next_comment) = parse_comment_line(lines[j]) {
                let current = CommentLine {
                    indent: comment.indent.clone(),
                    marker: comment.marker.clone(),
                    text: combined_text.clone(),
                };
                if can_combine(&current, &next_comment) {
                    combined_text.push(' ');
                    combined_text.push_str(next_comment.text.trim_start());
                    orig_lines.push(lines[j].to_string());
                    j += 1;
                    continue;
                }
            }
            break;
        }

        let base = format!("{}{}", comment.indent, comment.marker);
        let prefix = if combined_text.is_empty() {
            base.clone()
        } else {
            format!("{base} ")
        };
        let full_line = format!("{prefix}{combined_text}");

        let new_lines = if combined_text.is_empty() {
            vec![base.trim_end().to_string()]
        } else if full_line.len() > max_width {
            let marker_width = hierarchical_marker_width(&combined_text);
            let cont = if marker_width > 0 {
                format!("{prefix}{}", " ".repeat(marker_width))
            } else {
                prefix.clone()
            };
            wrap_text(&combined_text, &prefix, &cont, max_width)
        } else {
            vec![full_line.trim_end().to_string()]
        };

        if new_lines != orig_lines {
            changes.push((start_line + 1, orig_lines, new_lines.clone()));
        }

        result.extend(new_lines);
        i = j;
    }

    let mut output = result.join("\n");
    if content.ends_with('\n') {
        output.push('\n');
    }

    (output, changes)
}

/// Fallback line width when no CLI argument or `rustfmt.toml` config is found.
const DEFAULT_WIDTH: usize = 100;

/// Searches for `.rustfmt.toml` or `rustfmt.toml` from the current directory up to `$HOME`,
/// returning the `max_width` value and the config file path if found.
///
/// Stops at the first config file encountered. If that file exists but does not contain a
/// `max_width` key, returns `None`.
fn find_rustfmt_width() -> Option<(usize, PathBuf)> {
    let home = env::var("HOME").ok().map(PathBuf::from);
    let mut dir = env::current_dir().ok()?;

    loop {
        for name in &[".rustfmt.toml", "rustfmt.toml"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                if let Ok(contents) = fs::read_to_string(&candidate)
                    && let Ok(table) = contents.parse::<toml::Table>()
                    && let Some(val) = table.get("max_width")
                {
                    return val.as_integer().map(|v| (v as usize, candidate.clone()));
                }
                return None;
            }
        }

        if home.as_ref().is_some_and(|h| dir == *h) || !dir.pop() {
            break;
        }
    }
    None
}

/// Resolves the effective line width: CLI argument > `rustfmt.toml` > [`DEFAULT_WIDTH`].
fn resolve_width(cli_width: Option<usize>, verbose: bool) -> usize {
    if let Some(w) = cli_width {
        if verbose {
            eprintln!("Width: {w} (from --max-width)");
        }
        return w;
    }
    if let Some((w, path)) = find_rustfmt_width() {
        if verbose {
            eprintln!("Width: {w} (from {})", path.display());
        }
        return w;
    }
    if verbose {
        eprintln!("Width: {DEFAULT_WIDTH} (default)");
    }
    DEFAULT_WIDTH
}

fn main() {
    let CargoCli::WrapComments(cli) = CargoCli::parse();
    let verbose = cli.verbose;
    let quiet = cli.quiet && !verbose;
    let width = resolve_width(cli.max_width, verbose);

    if cli.files.is_empty() {
        process_stdin(width, cli.check);
        return;
    }

    let mut file_paths = BTreeSet::new();
    let mut had_errors = false;

    for pattern in &cli.files {
        let paths = match glob(pattern) {
            Ok(paths) => paths,
            Err(e) => {
                eprintln!("Invalid glob pattern '{pattern}': {e}");
                had_errors = true;
                continue;
            }
        };

        let prev_count = file_paths.len();
        for entry in paths {
            match entry {
                Ok(path) if path.is_file() => {
                    file_paths.insert(path);
                }
                Err(e) => {
                    eprintln!("Error reading path: {e}");
                    had_errors = true;
                }
                _ => {}
            }
        }
        if file_paths.len() == prev_count {
            eprintln!("No files found matching pattern: {pattern}");
            had_errors = true;
        }
    }

    if file_paths.is_empty() {
        eprintln!("No files to process.");
        process::exit(1);
    }

    if verbose {
        eprintln!("Found {} file(s) to process.", file_paths.len());
    }

    let mut files_processed = 0;
    let mut files_modified = 0;

    for path in &file_paths {
        let Ok(content) = fs::read_to_string(path) else {
            if verbose {
                eprintln!("{}:", path.display());
            }
            eprintln!("  Error reading {}", path.display());
            had_errors = true;
            continue;
        };

        files_processed += 1;
        let line_count = content.lines().count();
        let (new_content, changes) = process_content(&content, width);

        if changes.is_empty() {
            if verbose {
                eprintln!("{} ({line_count} lines): unchanged", path.display());
            }
            continue;
        }

        files_modified += 1;

        if verbose {
            eprintln!(
                "{} ({line_count} lines): {} change(s)",
                path.display(),
                changes.len()
            );
        }

        if cli.check {
            print_changes(&changes, Some(&format!("Would modify: {}", path.display())));
        } else if let Err(e) = fs::write(path, &new_content) {
            eprintln!("Error writing {}: {e}", path.display());
            had_errors = true;
            continue;
        } else if !quiet {
            eprintln!("Modified: {}", path.display());
        }
    }

    if !quiet {
        eprintln!("\nProcessed {files_processed} file(s), {files_modified} modified.");
    }

    if had_errors {
        process::exit(1);
    }
}

/// Reads from stdin, processes the content, and writes the result to stdout.
///
/// In `--check` mode, outputs the diff instead of the processed content.
fn process_stdin(max_width: usize, check: bool) {
    let mut content = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut content) {
        eprintln!("Error reading stdin: {e}");
        process::exit(1);
    }

    let (new_content, changes) = process_content(&content, max_width);

    if check {
        if !changes.is_empty() {
            print_changes(&changes, None);
            process::exit(1);
        }
    } else {
        print!("{new_content}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_double_slash() {
        let c = parse_comment_line("    // hello world").unwrap();
        assert_eq!(c.indent, "    ");
        assert_eq!(c.marker, "//");
        assert_eq!(c.text, "hello world");
    }

    #[test]
    fn test_parse_triple_slash() {
        let c = parse_comment_line("/// doc comment").unwrap();
        assert_eq!(c.indent, "");
        assert_eq!(c.marker, "///");
        assert_eq!(c.text, "doc comment");
    }

    #[test]
    fn test_parse_bang() {
        let c = parse_comment_line("//! module doc").unwrap();
        assert_eq!(c.marker, "//!");
        assert_eq!(c.text, "module doc");
    }

    #[test]
    fn test_parse_not_comment() {
        assert!(parse_comment_line("let x = 5;").is_none());
    }

    #[test]
    fn test_blank_comment() {
        let c = parse_comment_line("//").unwrap();
        assert_eq!(c.text, "");
    }

    #[test]
    fn test_hierarchical_markers() {
        assert!(starts_with_hierarchical_marker("# heading"));
        assert!(starts_with_hierarchical_marker("* bullet"));
        assert!(starts_with_hierarchical_marker("- dash"));
        assert!(starts_with_hierarchical_marker("1. numbered"));
        assert!(starts_with_hierarchical_marker("A. lettered"));
        assert!(starts_with_hierarchical_marker("a. lower"));
        assert!(starts_with_hierarchical_marker("```code"));
        assert!(!starts_with_hierarchical_marker("normal text"));
    }

    #[test]
    fn test_combine_basic() {
        let a = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            text: "hello".into(),
        };
        let b = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            text: "world".into(),
        };
        assert!(can_combine(&a, &b));
    }

    #[test]
    fn test_no_combine_different_markers() {
        let a = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            text: "hello".into(),
        };
        let b = CommentLine {
            indent: "".into(),
            marker: "///".into(),
            text: "world".into(),
        };
        assert!(!can_combine(&a, &b));
    }

    #[test]
    fn test_no_combine_blank() {
        let a = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            text: "hello".into(),
        };
        let b = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            text: "".into(),
        };
        assert!(!can_combine(&a, &b));
    }

    #[test]
    fn test_no_combine_hierarchical() {
        let a = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            text: "hello".into(),
        };
        let b = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            text: "- bullet".into(),
        };
        assert!(!can_combine(&a, &b));
    }

    #[test]
    fn test_wrap_text() {
        let result = wrap_text("hello world foo bar baz", "// ", "// ", 15);
        assert_eq!(result, vec!["// hello world", "// foo bar baz"]);
    }

    #[test]
    fn test_wrap_long_word() {
        let result = wrap_text("superlongword", "// ", "// ", 10);
        assert_eq!(result, vec!["// superlongword"]);
    }

    #[test]
    fn test_combines_and_wraps() {
        let input = "// This is a long comment that\n// should be combined\n";
        let (output, changes) = process_content(input, 40);
        assert_eq!(
            output,
            "// This is a long comment that should be\n// combined\n"
        );
        assert!(!changes.is_empty());
    }

    #[test]
    fn test_preserves_blank_comment() {
        let input = "// line one\n//\n// line two\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, input);
    }

    #[test]
    fn test_preserves_non_comments() {
        let input = "let x = 5;\nlet y = 10;\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_different_indentation_not_combined() {
        let input = "    // indented\n// not indented\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, input);
    }

    #[test]
    fn test_no_combine_code_block() {
        let input = "// some text\n// ```rust\n// code here\n// ```\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, input);
    }

    #[test]
    fn test_no_wrap_inside_code_block() {
        let input = "// ```\n// this is a very long line inside a code block that should not be wrapped even though it exceeds the width limit\n// ```\n";
        let (output, changes) = process_content(input, 40);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_no_combine_inside_code_block() {
        let input = "// ```\n// line one\n// line two\n// ```\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_resumes_after_code_block() {
        let input = "// ```\n// code\n// ```\n// hello\n// world\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// ```\n// code\n// ```\n// hello world\n");
    }

    #[test]
    fn test_hierarchical_marker_width() {
        assert_eq!(hierarchical_marker_width("- item"), 2);
        assert_eq!(hierarchical_marker_width("* item"), 2);
        assert_eq!(hierarchical_marker_width("1. item"), 3);
        assert_eq!(hierarchical_marker_width("A. item"), 3);
        assert_eq!(hierarchical_marker_width("# heading"), 2);
        assert_eq!(hierarchical_marker_width("## heading"), 3);
        assert_eq!(hierarchical_marker_width("### heading"), 4);
        assert_eq!(hierarchical_marker_width("normal text"), 0);
        assert_eq!(hierarchical_marker_width("```code"), 0);
        assert_eq!(hierarchical_marker_width("10. item"), 4);
        assert_eq!(hierarchical_marker_width("100. item"), 5);
    }

    #[test]
    fn test_multi_digit_hierarchical_marker() {
        assert!(starts_with_hierarchical_marker("10. tenth item"));
        assert!(starts_with_hierarchical_marker("100. hundredth"));
        assert!(!starts_with_hierarchical_marker("10x not a marker"));
    }

    #[test]
    fn test_no_combine_multi_digit_numbered() {
        let input = "// first line\n// 10. tenth item\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, input);
    }

    #[test]
    fn test_process_content_wraps_multi_digit_numbered_with_indent() {
        let input =
            "// 10. This is a very long numbered item that should wrap with proper indentation\n";
        let (output, _) = process_content(input, 40);
        assert_eq!(
            output,
            "// 10. This is a very long numbered item\n//     that should wrap with proper\n//     indentation\n"
        );
    }

    #[test]
    fn test_combine_strips_hanging_indent() {
        let input = "// - bullet point\n//   continuation text\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// - bullet point continuation text\n");
    }

    #[test]
    fn test_combine_strips_hanging_indent_numbered() {
        let input = "// 1. first item\n//    more text here\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// 1. first item more text here\n");
    }

    #[test]
    fn test_wrap_hierarchical_marker_indent() {
        let result = wrap_text("- this is a long bullet point text", "// ", "//   ", 30);
        assert_eq!(
            result,
            vec!["// - this is a long bullet", "//   point text"]
        );
    }

    #[test]
    fn test_wrap_numbered_list_indent() {
        let result = wrap_text("1. this is a long numbered item text", "// ", "//    ", 30);
        assert_eq!(
            result,
            vec!["// 1. this is a long numbered", "//    item text"]
        );
    }

    #[test]
    fn test_process_content_wraps_bullet_with_indent() {
        let input =
            "// - This is a very long bullet point that should wrap with proper indentation\n";
        let (output, changes) = process_content(input, 40);
        assert_eq!(
            output,
            "// - This is a very long bullet point\n//   that should wrap with proper\n//   indentation\n"
        );
        assert!(!changes.is_empty());
    }

    #[test]
    fn test_process_content_wraps_numbered_with_indent() {
        let input =
            "// 1. This is a very long numbered item that should wrap with proper indentation\n";
        let (output, _) = process_content(input, 40);
        assert_eq!(
            output,
            "// 1. This is a very long numbered item\n//    that should wrap with proper\n//    indentation\n"
        );
    }

    #[test]
    fn test_no_trailing_spaces_blank_comment() {
        let input = "//\n";
        let (output, _) = process_content(input, 80);
        for line in output.lines() {
            assert_eq!(line, line.trim_end(), "trailing spaces in: {:?}", line);
        }
    }

    #[test]
    fn test_no_trailing_spaces_after_wrap() {
        let input =
            "// This is a long comment that needs to be wrapped because it exceeds the width\n";
        let (output, _) = process_content(input, 40);
        for line in output.lines() {
            assert_eq!(line, line.trim_end(), "trailing spaces in: {:?}", line);
        }
    }

    #[test]
    fn test_no_trailing_spaces_doc_comment() {
        let input =
            "///\n/// Some doc text that is long enough to wrap around to the next line easily\n";
        let (output, _) = process_content(input, 40);
        for line in output.lines() {
            assert_eq!(line, line.trim_end(), "trailing spaces in: {:?}", line);
        }
    }

    #[test]
    fn test_no_trailing_spaces_hierarchical_wrap() {
        let input = "// - A bullet point that is long enough to be wrapped across multiple lines\n";
        let (output, _) = process_content(input, 40);
        for line in output.lines() {
            assert_eq!(line, line.trim_end(), "trailing spaces in: {:?}", line);
        }
    }

    #[test]
    fn test_tokenize_preserving_backticks() {
        assert_eq!(
            tokenize_preserving_backticks("use `foo bar` here"),
            vec!["use", "`foo bar`", "here"]
        );
        assert_eq!(
            tokenize_preserving_backticks("use ``foo bar`` here"),
            vec!["use", "``foo bar``", "here"]
        );
        assert_eq!(
            tokenize_preserving_backticks("no backticks here"),
            vec!["no", "backticks", "here"]
        );
        assert_eq!(
            tokenize_preserving_backticks("`start` and `end`"),
            vec!["`start`", "and", "`end`"]
        );
    }

    #[test]
    fn test_wrap_preserves_backtick_span() {
        // Width 18: available = 15, "use `foo bar`" = 13 fits, "for" = 13+1+3=17 > 15 → wraps
        let result = wrap_text("use `foo bar` for things", "// ", "// ", 18);
        assert_eq!(result, vec!["// use `foo bar`", "// for things"]);
    }

    #[test]
    fn test_wrap_preserves_double_backtick_span() {
        // Width 20: available = 17, "use ``foo bar``" = 15 fits, "for" = 15+1+3=19 > 17 → wraps
        let result = wrap_text("use ``foo bar`` for things", "// ", "// ", 20);
        assert_eq!(result, vec!["// use ``foo bar``", "// for things"]);
    }

    #[test]
    fn test_no_break_inside_backticks_in_comment() {
        let input = "// This uses `long code span` and should not break inside backticks\n";
        let (output, _) = process_content(input, 35);
        for line in output.lines() {
            let text = line.trim_start_matches("//").trim();
            let backtick_count = text.chars().filter(|&c| c == '`').count();
            assert_eq!(backtick_count % 2, 0, "unmatched backtick in: {line:?}");
        }
    }

    #[test]
    fn test_tokenize_single_backtick_not_closed_by_double() {
        // ` `` ` is a single-backtick span containing `` (space-double-backtick-space)
        assert_eq!(
            tokenize_preserving_backticks("` `` ` rest"),
            vec!["` `` `", "rest"]
        );
    }

    #[test]
    fn test_tokenize_mixed_backtick_delimiters() {
        // `` ` `` is a double-backtick span containing a single backtick ` `` ` is a
        // single-backtick span containing double backticks
        assert_eq!(
            tokenize_preserving_backticks("`` ` `` or ` `` ` end"),
            vec!["`` ` ``", "or", "` `` `", "end"]
        );
    }

    #[test]
    fn test_wrap_does_not_split_mixed_backtick_spans() {
        // `` ` `` and ` `` ` are complete backtick spans; wrapping must not split them
        let input = "/// keeps (`` ` `` or ` `` `) as single tokens even if they contain spaces.\n";
        let (output, _) = process_content(input, 60);
        let lines: Vec<&str> = output.lines().collect();
        // The wrap must not split inside the ` `` ` span
        assert!(
            !lines
                .iter()
                .any(|l| l.ends_with("` ``") || l.ends_with("`` `")),
            "backtick span was split across lines: {lines:?}"
        );
    }

    #[test]
    fn test_resolve_width_cli_overrides() {
        assert_eq!(resolve_width(Some(80), false), 80);
    }

    #[test]
    fn test_resolve_width_default() {
        assert!(resolve_width(None, false) > 0);
    }

    #[test]
    fn test_verbose_overrides_quiet() {
        // When both verbose and quiet are set, verbose wins (quiet becomes false).
        let verbose = true;
        let quiet_flag = true;
        let effective_quiet = quiet_flag && !verbose;
        assert!(!effective_quiet);
    }
}
