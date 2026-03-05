use clap::Parser;
use glob::glob;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

#[derive(Parser)]
#[command(name = "comment-reformatter")]
#[command(about = "Reformat source code comments to wrap at a specified width")]
struct Cli {
    /// Files or glob patterns to process
    #[arg(required = true)]
    files: Vec<String>,

    /// Maximum line width
    #[arg(long)]
    width: Option<usize>,

    /// Preview mode - show what would change without modifying files
    #[arg(long)]
    check: bool,
}

#[derive(Debug, Clone)]
struct CommentLine {
    indent: String,
    marker: String,
    text: String,
}

/// Detect if a line is a comment, returning the parsed parts.
fn parse_comment_line(line: &str) -> Option<CommentLine> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = line.trim_start();

    let markers = ["///", "//!", "//"];
    for marker in &markers {
        if trimmed.starts_with(marker) {
            let rest = &trimmed[marker.len()..];
            let text = if rest.starts_with(' ') {
                &rest[1..]
            } else {
                rest
            };
            return Some(CommentLine {
                indent: indent.to_string(),
                marker: marker.to_string(),
                text: text.to_string(),
            });
        }
    }
    None
}

fn starts_with_hierarchical_marker(text: &str) -> bool {
    if text.starts_with('#')
        || text.starts_with('*')
        || text.starts_with('-')
        || text.starts_with("```")
    {
        return true;
    }

    let mut chars = text.chars();
    if let Some(first) = chars.next() {
        if first.is_ascii_digit() || first.is_ascii_alphabetic() {
            if let Some(second) = chars.next() {
                if second == '.' {
                    return true;
                }
            }
        }
    }
    false
}

fn can_combine(current: &CommentLine, next: &CommentLine) -> bool {
    if current.marker != next.marker || current.indent != next.indent {
        return false;
    }
    if current.text.is_empty() || next.text.is_empty() {
        return false;
    }
    if starts_with_hierarchical_marker(&next.text) {
        return false;
    }
    if current.text.contains("```") {
        return false;
    }
    true
}

fn wrap_text(text: &str, prefix: &str, max_width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![prefix.trim_end().to_string()];
    }

    let available = if max_width > prefix.len() {
        max_width - prefix.len()
    } else {
        1
    };

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![prefix.trim_end().to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in &words {
        if current_line.is_empty() {
            current_line.push_str(word);
        } else if current_line.len() + 1 + word.len() <= available {
            current_line.push(' ');
            current_line.push_str(word);
        } else {
            lines.push(format!("{}{}", prefix, current_line));
            current_line = word.to_string();
        }
    }

    if !current_line.is_empty() {
        lines.push(format!("{}{}", prefix, current_line));
    }

    lines
}

fn process_content(content: &str, max_width: usize) -> (String, Vec<(usize, Vec<String>, Vec<String>)>) {
    let lines: Vec<&str> = content.lines().collect();
    let mut result: Vec<String> = Vec::new();
    let mut changes: Vec<(usize, Vec<String>, Vec<String>)> = Vec::new();
    let mut i = 0;
    let mut in_code_block = false;

    while i < lines.len() {
        if let Some(comment) = parse_comment_line(lines[i]) {
            // Track code block state
            if comment.text.starts_with("```") {
                in_code_block = !in_code_block;
                result.push(lines[i].to_string());
                i += 1;
                continue;
            }

            // Pass through lines inside code blocks unchanged
            if in_code_block {
                result.push(lines[i].to_string());
                i += 1;
                continue;
            }

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
                        combined_text.push_str(&next_comment.text);
                        orig_lines.push(lines[j].to_string());
                        j += 1;
                        continue;
                    }
                }
                break;
            }

            let prefix = if combined_text.is_empty() {
                format!("{}{}", comment.indent, comment.marker)
            } else {
                format!("{}{} ", comment.indent, comment.marker)
            };

            let full_line = format!("{}{}", prefix, combined_text);

            let new_lines = if !combined_text.is_empty() && full_line.len() > max_width {
                wrap_text(&combined_text, &prefix, max_width)
            } else if combined_text.is_empty() {
                vec![format!("{}{}", comment.indent, comment.marker)]
            } else {
                vec![full_line]
            };

            if new_lines.len() != orig_lines.len()
                || new_lines.iter().zip(orig_lines.iter()).any(|(a, b)| a != b)
            {
                changes.push((start_line + 1, orig_lines, new_lines.clone()));
            }

            result.extend(new_lines);
            i = j;
        } else {
            result.push(lines[i].to_string());
            i += 1;
        }
    }

    let mut output = result.join("\n");
    if content.ends_with('\n') {
        output.push('\n');
    }

    (output, changes)
}

const DEFAULT_WIDTH: usize = 100;

/// Search for .rustfmt.toml or rustfmt.toml from the current directory up to $HOME.
fn find_rustfmt_width() -> Option<usize> {
    let home = env::var("HOME").ok().map(PathBuf::from);
    let mut dir = env::current_dir().ok()?;

    loop {
        for name in &[".rustfmt.toml", "rustfmt.toml"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                if let Ok(contents) = fs::read_to_string(&candidate) {
                    if let Ok(table) = contents.parse::<toml::Table>() {
                        if let Some(val) = table.get("max_width") {
                            return val.as_integer().map(|v| v as usize);
                        }
                    }
                }
                // Found a rustfmt config but no max_width in it
                return None;
            }
        }

        // Stop if we've reached home or the filesystem root
        if home.as_ref().is_some_and(|h| dir == *h) {
            break;
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

fn resolve_width(cli_width: Option<usize>) -> usize {
    if let Some(w) = cli_width {
        return w;
    }
    if let Some(w) = find_rustfmt_width() {
        return w;
    }
    DEFAULT_WIDTH
}

fn main() {
    let cli = Cli::parse();
    let width = resolve_width(cli.width);

    let mut file_paths = BTreeSet::new();
    let mut had_errors = false;

    for pattern in &cli.files {
        match glob(pattern) {
            Ok(paths) => {
                let mut matched = false;
                for entry in paths {
                    match entry {
                        Ok(path) => {
                            if path.is_file() {
                                file_paths.insert(path);
                                matched = true;
                            }
                        }
                        Err(e) => {
                            eprintln!("Error reading path: {}", e);
                            had_errors = true;
                        }
                    }
                }
                if !matched {
                    eprintln!("No files found matching pattern: {}", pattern);
                    had_errors = true;
                }
            }
            Err(e) => {
                eprintln!("Invalid glob pattern '{}': {}", pattern, e);
                had_errors = true;
            }
        }
    }

    if file_paths.is_empty() {
        eprintln!("No files to process.");
        process::exit(1);
    }

    let mut files_processed = 0;
    let mut files_modified = 0;

    for path in &file_paths {
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error reading {}: {}", path.display(), e);
                had_errors = true;
                continue;
            }
        };

        files_processed += 1;
        let (new_content, changes) = process_content(&content, width);

        if changes.is_empty() {
            continue;
        }

        files_modified += 1;

        if cli.check {
            println!("Would modify: {}", path.display());
            for (line_num, removed, added) in &changes {
                println!("  Line {}:", line_num);
                for r in removed {
                    println!("    - {}", r);
                }
                for a in added {
                    println!("    + {}", a);
                }
            }
        } else {
            if let Err(e) = fs::write(path, &new_content) {
                eprintln!("Error writing {}: {}", path.display(), e);
                had_errors = true;
                continue;
            }
            println!("Modified: {}", path.display());
        }
    }

    println!(
        "\nProcessed {} file(s), {} modified.",
        files_processed, files_modified
    );

    if had_errors {
        process::exit(1);
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
        let a = CommentLine { indent: "".into(), marker: "//".into(), text: "hello".into() };
        let b = CommentLine { indent: "".into(), marker: "//".into(), text: "world".into() };
        assert!(can_combine(&a, &b));
    }

    #[test]
    fn test_no_combine_different_markers() {
        let a = CommentLine { indent: "".into(), marker: "//".into(), text: "hello".into() };
        let b = CommentLine { indent: "".into(), marker: "///".into(), text: "world".into() };
        assert!(!can_combine(&a, &b));
    }

    #[test]
    fn test_no_combine_blank() {
        let a = CommentLine { indent: "".into(), marker: "//".into(), text: "hello".into() };
        let b = CommentLine { indent: "".into(), marker: "//".into(), text: "".into() };
        assert!(!can_combine(&a, &b));
    }

    #[test]
    fn test_no_combine_hierarchical() {
        let a = CommentLine { indent: "".into(), marker: "//".into(), text: "hello".into() };
        let b = CommentLine { indent: "".into(), marker: "//".into(), text: "- bullet".into() };
        assert!(!can_combine(&a, &b));
    }

    #[test]
    fn test_wrap_text() {
        let result = wrap_text("hello world foo bar baz", "// ", 15);
        assert_eq!(result, vec!["// hello world", "// foo bar baz"]);
    }

    #[test]
    fn test_wrap_long_word() {
        let result = wrap_text("superlongword", "// ", 10);
        assert_eq!(result, vec!["// superlongword"]);
    }

    #[test]
    fn test_combines_and_wraps() {
        let input = "// This is a long comment that\n// should be combined\n";
        let (output, changes) = process_content(input, 40);
        assert_eq!(output, "// This is a long comment that should be\n// combined\n");
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
    fn test_resolve_width_cli_overrides() {
        assert_eq!(resolve_width(Some(80)), 80);
    }

    #[test]
    fn test_resolve_width_default() {
        // Without a rustfmt.toml in the ancestor dirs, falls back to DEFAULT_WIDTH
        assert!(resolve_width(None) > 0);
    }
}
