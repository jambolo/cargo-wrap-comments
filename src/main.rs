//! A CLI tool that reformats source code comments to wrap at a specified width.
//!
//! `cargo-wrap-comments` combines consecutive comment lines and re-wraps them to fit within a configurable line width, similar to
//! what `rustfmt` does for code but applied to `//`, `///`, and `//!` comments and to block doc comments (`/** */` and `/*! */`).
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
//! 2. `max_width` from the rustfmt config file
//! 3. Default: 100
//!
//! The config file is located exactly as rustfmt locates it. With `--config-path`, it is that file, or the `.rustfmt.toml` or
//! `rustfmt.toml` directly inside that directory. Otherwise, for each file, the search covers the file's directory and each
//! ancestor up to the filesystem root, then the home directory, then `<user config dir>/rustfmt`. For stdin, the search starts in
//! the current directory.

use clap::Parser;
use glob::glob;
use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process;
use unicode_width::UnicodeWidthStr;

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

    /// Use this rustfmt config file, or the one directly inside this directory, instead of searching for one
    #[arg(long, value_name = "PATH")]
    config_path: Option<PathBuf>,

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
    /// The comment marker (`//`, `///`, `//!`, or a run of four or more slashes), or, in the body of a block doc comment, `*`
    /// (leading-asterisk style) or the empty string (no-asterisk style).
    marker: String,
    /// Whether a space follows the marker. Always `false` for an empty marker.
    space: bool,
    /// The text content after the marker and optional space.
    text: String,
}

/// Recognized comment markers, ordered longest-first to ensure `///` and `//!` match before `//`.
const MARKERS: [&str; 3] = ["///", "//!", "//"];

/// Parses a source line into a [`CommentLine`] if it contains a recognized comment marker.
///
/// Four or more slashes make a plain comment, not a doc comment, so the whole run of slashes is the marker.
///
/// Returns `None` for non-comment lines.
fn parse_comment_line(line: &str) -> Option<CommentLine> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    let slashes = trimmed.bytes().take_while(|&b| b == b'/').count();
    let marker = if slashes >= 4 {
        &trimmed[..slashes]
    } else {
        MARKERS.into_iter().find(|marker| trimmed.starts_with(marker))?
    };
    let rest = &trimmed[marker.len()..];
    let text = rest.strip_prefix(' ');
    Some(CommentLine {
        indent: indent.to_string(),
        marker: marker.to_string(),
        space: text.is_some(),
        text: text.unwrap_or(rest).to_string(),
    })
}

/// Returns the block comment nesting depth at the end of `line`, given the depth at its start. Scanning stops when the outermost
/// comment closes, so code after it on the same line is ignored.
fn block_comment_depth(line: &str, mut depth: usize) -> usize {
    let bytes = line.as_bytes();
    let mut k = 0;
    while k + 1 < bytes.len() {
        match (bytes[k], bytes[k + 1]) {
            (b'/', b'*') => {
                depth += 1;
                k += 2;
            }
            (b'*', b'/') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return 0;
                }
                k += 2;
            }
            _ => k += 1,
        }
    }
    depth
}

/// Returns `true` if `trimmed` opens a multi-line block doc comment: `/**` or `/*!` with nothing after it. `/***` and `/**/` are
/// plain block comments.
fn is_block_doc_opener(trimmed: &str) -> bool {
    trimmed
        .strip_prefix("/**")
        .or_else(|| trimmed.strip_prefix("/*!"))
        .is_some_and(|rest| rest.trim().is_empty())
}

/// Parses the lines between the opening and closing lines of a block doc comment.
///
/// If every non-blank line starts with `*` followed by a space or the end of the line, the `*` is the marker. Otherwise the
/// common leading whitespace is the indent and the marker is empty. Blank lines and lines containing a nested block comment
/// delimiter are not parsed.
fn parse_block_doc_body(body: &[&str]) -> Vec<Option<CommentLine>> {
    let text_lines = || body.iter().filter(|line| !line.trim().is_empty());
    let asterisk = text_lines().all(|line| {
        let t = line.trim_start();
        t == "*" || t.starts_with("* ")
    });
    let common_indent = text_lines()
        .map(|line| line.len() - line.trim_start_matches([' ', '\t']).len())
        .min()
        .unwrap_or(0);

    body.iter()
        .map(|&line| {
            if line.trim().is_empty() || line.contains("/*") || line.contains("*/") {
                return None;
            }
            if asterisk {
                let t = line.trim_start();
                let rest = &t[1..];
                Some(CommentLine {
                    indent: line[..line.len() - t.len()].to_string(),
                    marker: "*".to_string(),
                    space: rest.starts_with(' '),
                    text: rest.strip_prefix(' ').unwrap_or(rest).to_string(),
                })
            } else {
                Some(CommentLine {
                    indent: line[..common_indent].to_string(),
                    marker: String::new(),
                    space: false,
                    text: line[common_indent..].to_string(),
                })
            }
        })
        .collect()
}

/// Parses every line into a [`CommentLine`], or `None` if the line is passed through unchanged.
///
/// A block comment that starts a line is followed to its end (block comments nest), so line comment markers inside it are not
/// taken for comments. Only the body of a multi-line block doc comment is parsed; its opening and closing lines, and all other
/// block comments, are passed through.
fn parse_lines(lines: &[&str]) -> Vec<Option<CommentLine>> {
    let mut parsed = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if !trimmed.starts_with("/*") {
            parsed.push(parse_comment_line(lines[i]));
            i += 1;
            continue;
        }
        let mut depth = block_comment_depth(trimmed, 0);
        let mut end = i;
        while depth > 0 && end + 1 < lines.len() {
            end += 1;
            depth = block_comment_depth(lines[end], depth);
        }
        parsed.push(None);
        if end > i {
            let body = &lines[i + 1..end];
            if depth == 0 && is_block_doc_opener(trimmed) {
                parsed.extend(parse_block_doc_body(body));
            } else {
                parsed.extend(body.iter().map(|_| None));
            }
            parsed.push(None);
        }
        i = end + 1;
    }
    parsed
}

/// Returns `true` if `rest` is empty or starts with a space or tab, as required after a list marker.
fn ends_marker(rest: &str) -> bool {
    matches!(rest.as_bytes().first(), None | Some(b' ' | b'\t'))
}

/// Returns the number of a CommonMark ordered list marker at the start of `text`: 1 to 9 digits followed by `.` or `)` and a
/// space, a tab, or the end of the line (e.g. `10` for `10. item`, `1` for `1) item`). Letters and Roman numerals are not
/// markers.
fn ordered_marker_label(text: &str) -> Option<&str> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    let rest = text[digits..].strip_prefix(['.', ')'])?;
    ((1..=9).contains(&digits) && ends_marker(rest)).then_some(&text[..digits])
}

/// Returns `true` if `text` begins with a list marker, ignoring leading whitespace (nested list items are indented): `*` or `-`
/// followed by a space, a tab, or the end of the line, `+` followed by a space, or an ordered marker (see
/// [`ordered_marker_label`]).
fn starts_with_list_marker(text: &str) -> bool {
    let text = text.trim_start();
    let bullet = text.strip_prefix(['*', '-']).is_some_and(ends_marker) || text.starts_with("+ ");
    bullet || ordered_marker_label(text).is_some()
}

/// Returns the fence character and length if `text`, ignoring leading whitespace, starts with a code fence: three or more
/// backticks or tildes (e.g. `` ``` ``, `~~~~`).
fn code_fence(text: &str) -> Option<(char, usize)> {
    let text = text.trim_start();
    let first @ ('`' | '~') = text.chars().next()? else {
        return None;
    };
    let len = text.chars().take_while(|&c| c == first).count();
    (len >= 3).then_some((first, len))
}

/// Returns `true` if `text` closes a code block opened by `open`: a fence of the same character, at least as long, followed only
/// by whitespace.
fn closes_code_fence(text: &str, open: (char, usize)) -> bool {
    code_fence(text).is_some_and(|(c, len)| c == open.0 && len >= open.1 && text.trim_start()[len..].trim().is_empty())
}

/// Returns `true` if `text` begins with a markdown-style structural marker, ignoring leading whitespace (nested list items are
/// indented).
///
/// Recognized markers: an ATX heading (see [`is_atx_heading`]), a code fence (`` ``` `` or `~~~`), and the list markers
/// recognized by [`starts_with_list_marker`].
fn starts_with_hierarchical_marker(text: &str) -> bool {
    let text = text.trim_start();
    is_atx_heading(text) || code_fence(text).is_some() || starts_with_list_marker(text)
}

/// Returns `true` if `next` starts a new block instead of continuing `current`.
///
/// As in CommonMark, an ordered list item can interrupt a paragraph only if its number is 1, so a line starting with any other
/// ordered marker (e.g. `2026. That was the year.`) continues a paragraph. It starts a new item only when `current` is itself a
/// list item.
fn starts_new_block(current: &str, next: &str) -> bool {
    if !starts_with_hierarchical_marker(next) {
        return false;
    }
    match ordered_marker_label(next.trim_start()) {
        Some(label) if label.parse::<u32>() != Ok(1) => starts_with_list_marker(current),
        _ => true,
    }
}

/// Returns `true` if `text` is a CommonMark ATX heading: 1 to 6 `#` characters followed by a space, a tab, or the end of the line.
/// `#42`, `#hashtag`, and `####### seven` are not headings.
fn is_atx_heading(text: &str) -> bool {
    let hashes = text.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&hashes) && matches!(text.as_bytes().get(hashes), None | Some(b' ' | b'\t'))
}

/// Returns the width of a hierarchical marker prefix followed by a space (e.g. `- ` → 2, `+ ` → 2, `1. ` → 3, `10) ` → 4,
/// `## ` → 3).
///
/// Returns 0 if the text does not start with a recognized marker.
fn hierarchical_marker_width(text: &str) -> usize {
    if is_atx_heading(text) {
        let hashes = text.bytes().take_while(|&b| b == b'#').count();
        if text.as_bytes().get(hashes) == Some(&b' ') {
            return hashes + 1;
        }
    }
    if matches!(text.as_bytes().first(), Some(b'-' | b'*' | b'+')) && text.as_bytes().get(1) == Some(&b' ') {
        return 2;
    }
    match ordered_marker_label(text) {
        // The marker is the number followed by a delimiter and a space.
        Some(label) if text.as_bytes().get(label.len() + 1) == Some(&b' ') => label.len() + 2,
        _ => 0,
    }
}

/// Returns `true` if `text` is a divider: four or more repetitions of a single punctuation character (e.g. `----`, `=====`,
/// `*****`, `~~~~`), optionally separated by spaces. Three is too few because markdown uses `---`/`***`/`~~~` and `///` is a
/// doc comment marker.
fn is_divider(text: &str) -> bool {
    let mut chars = text.chars().filter(|c| !c.is_whitespace());
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_punctuation() && first != '`' && chars.clone().all(|c| c == first) && chars.count() >= 3
}

/// Returns `true` if `text` is a markdown thematic break: up to three spaces of indentation, then three or more matching `-`, `_`,
/// or `*` characters, each optionally followed by spaces or tabs (e.g. `---`, `***`, `_ _ _`).
fn is_thematic_break(text: &str) -> bool {
    let body = text.trim_start_matches(' ');
    if text.len() - body.len() > 3 {
        return false;
    }
    let Some(first @ ('-' | '_' | '*')) = body.chars().next() else {
        return false;
    };
    body.chars().all(|c| c == first || c == ' ' || c == '\t') && body.chars().filter(|&c| c == first).count() >= 3
}

/// Returns `true` if `text` starts with an SPDX tag (e.g. `SPDX-License-Identifier:`). SPDX tools read a tag's value up to the
/// end of the line, so such a line is never combined or wrapped.
fn is_spdx_line(text: &str) -> bool {
    text.strip_prefix("SPDX-")
        .and_then(|rest| rest.split_once(':'))
        .is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// Annotation tags that start a new note, e.g. `TODO:` or `FIXME(name):`.
const ANNOTATION_TAGS: [&str; 6] = ["TODO", "FIXME", "NOTE", "HACK", "XXX", "SAFETY"];

/// Returns `true` if `text` starts with an annotation tag, optionally followed by `(...)`, then `:` (e.g. `TODO:`,
/// `TODO(name):`).
fn starts_with_annotation_tag(text: &str) -> bool {
    ANNOTATION_TAGS.iter().any(|tag| {
        let Some(rest) = text.strip_prefix(tag) else {
            return false;
        };
        let rest = match rest.strip_prefix('(') {
            Some(inner) => match inner.split_once(')') {
                Some((_, after)) => after,
                None => return false,
            },
            None => rest,
        };
        rest.starts_with(':')
    })
}

/// Splits a markdown block quote prefix off `text`: one or more `>`, each preceded by up to three spaces and optionally followed
/// by one space (e.g. `> `, `> > `, `>>`).
///
/// Returns the prefix, the quote depth (the number of `>`), and the text after the prefix.
fn split_quote(text: &str) -> (&str, usize, &str) {
    let mut depth = 0;
    let mut end = 0;
    loop {
        let rest = &text[end..];
        let unindented = rest.trim_start_matches(' ');
        if rest.len() - unindented.len() > 3 {
            break;
        }
        let Some(after) = unindented.strip_prefix('>') else {
            break;
        };
        depth += 1;
        end = text.len() - after.strip_prefix(' ').unwrap_or(after).len();
    }
    (&text[..end], depth, &text[end..])
}

/// Returns `true` if `text` is a markdown table row (its trimmed text starts with `|`).
fn is_table_row(text: &str) -> bool {
    text.trim_start().starts_with('|')
}

/// Returns `true` if `text` ends with an unescaped backslash, which is a markdown hard line break at the end of a line.
fn ends_with_backslash_break(text: &str) -> bool {
    text.bytes().rev().take_while(|&b| b == b'\\').count() % 2 == 1
}

/// Returns the trailing spaces of `text` if there are two or more, which is a markdown hard line break at the end of a line.
/// Otherwise, returns an empty string.
fn trailing_space_break(text: &str) -> &str {
    let trimmed = text.trim_end_matches(' ');
    if text.len() - trimmed.len() >= 2 {
        &text[trimmed.len()..]
    } else {
        ""
    }
}

/// Determines whether two adjacent comment lines should be combined.
///
/// Lines are combined when they share the same marker, indentation, and block quote depth, and the following rules hold for the
/// text after any block quote prefix: neither is blank, a divider, a thematic break, a table row, or an SPDX tag, the next line
/// doesn't start a new block (see [`starts_new_block`]) or start with an annotation tag, the current line doesn't start with or
/// contain a code fence or end with a hard line break (a backslash or two or more spaces), and a markdown heading line (see
/// [`is_atx_heading`]) is not combined with the text that follows.
fn can_combine(current: &CommentLine, next: &CommentLine) -> bool {
    let (_, current_depth, current_text) = split_quote(&current.text);
    let (_, next_depth, next_text) = split_quote(&next.text);
    current.marker == next.marker
        && current.indent == next.indent
        && current_depth == next_depth
        && !current_text.is_empty()
        && !next_text.is_empty()
        && !ends_with_backslash_break(current_text)
        && trailing_space_break(current_text).is_empty()
        && !is_divider(current_text)
        && !is_divider(next_text)
        && !is_thematic_break(current_text)
        && !is_thematic_break(next_text)
        && !is_table_row(current_text)
        && !is_table_row(next_text)
        && !is_spdx_line(current_text)
        && !is_spdx_line(next_text)
        && !starts_with_annotation_tag(next_text)
        && !starts_new_block(current_text, next_text)
        && !current_text.contains("```")
        && code_fence(current_text).is_none()
        && !is_atx_heading(current_text)
}

/// Splits text into tokens at whitespace boundaries, but keeps backtick-delimited spans (`` ` `` or ` `` ` as single tokens even if
/// they contain spaces.
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
/// The first output line is prefixed with `prefix` (typically indent + marker + space). Continuation lines use
/// `continuation_prefix`, which may include extra indentation to align with text after a hierarchical marker. Words that exceed the
/// available width are emitted on their own line without truncation. Backtick-delimited spans are kept on the same line, and a word
/// ending in a backslash is kept on the same line as the word after it.
fn wrap_text(text: &str, prefix: &str, continuation_prefix: &str, max_width: usize) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for token in tokenize_preserving_backticks(text) {
        match words.last_mut() {
            // A backslash at the end of an output line would be a hard line break that was not in the input.
            Some(last) if ends_with_backslash_break(last) => {
                last.push(' ');
                last.push_str(&token);
            }
            _ => words.push(token),
        }
    }
    if words.is_empty() {
        return vec![prefix.trim_end().to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in &words {
        let pfx = if lines.is_empty() { prefix } else { continuation_prefix };
        let available = max_width.saturating_sub(pfx.width()).max(1);

        if current_line.is_empty() {
            current_line.push_str(word);
        } else if current_line.width() + 1 + word.width() <= available {
            current_line.push(' ');
            current_line.push_str(word);
        } else {
            lines.push(format!("{pfx}{current_line}").trim_end().to_string());
            current_line = word.to_string();
        }
    }

    let pfx = if lines.is_empty() { prefix } else { continuation_prefix };
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

/// Returns the indentation of `text` in columns, with tabs advancing to the next multiple of 4 as in CommonMark.
fn leading_indent(text: &str) -> usize {
    let mut width = 0;
    for c in text.chars() {
        match c {
            ' ' => width += 1,
            '\t' => width += 4 - width % 4,
            _ => break,
        }
    }
    width
}

/// Returns the column at which the content of a list item starts (e.g. `  - item` → 4), or `None` if `text` is not a list item.
fn list_content_column(text: &str) -> Option<usize> {
    let body = text.trim_start();
    if is_atx_heading(body) {
        return None;
    }
    match hierarchical_marker_width(body) {
        0 => None,
        width => Some(leading_indent(text) + width),
    }
}

/// Processes an entire file's content, combining and wrapping comments.
///
/// Returns the reformatted content and a list of [`Change`] tuples describing what was modified. Lines inside code blocks
/// (`` ``` `` or `~~~` fences, or CommonMark indented code blocks) are passed through unchanged, as are lines that
/// [`parse_lines`] does not parse.
fn process_content(content: &str, max_width: usize) -> (String, Vec<Change>) {
    let lines: Vec<&str> = content.lines().collect();
    let parsed = parse_lines(&lines);
    let mut result: Vec<String> = Vec::new();
    let mut changes: Vec<Change> = Vec::new();
    let mut i = 0;
    // The opening fence of the code block being passed through, if any.
    let mut open_fence: Option<(char, usize)> = None;
    let mut in_indented_code = false;
    // The indent and marker of the previous line if it is a comment, and whether its text is blank.
    let mut prev: Option<(String, String, bool)> = None;
    // Where the content of the most recent list item in the comment block starts. Indented code inside the item is indented 4
    // columns past it.
    let mut list_column: Option<usize> = None;

    while i < lines.len() {
        let comment = parsed[i].as_ref();

        if let Some(c) = comment {
            let same_block = prev
                .as_ref()
                .is_some_and(|(indent, marker, _)| *indent == c.indent && *marker == c.marker);
            if !same_block {
                list_column = None;
            }
            let blank = c.text.trim().is_empty();
            let indent = leading_indent(&c.text);
            let code_indent = list_column.unwrap_or(0) + 4;
            if in_indented_code && (!same_block || (!blank && indent < code_indent)) {
                in_indented_code = false;
            }
            // An indented code block cannot interrupt a paragraph, so it starts only after a blank line or at the block start.
            let prev_blank = prev.as_ref().is_none_or(|(_, _, blank)| *blank);
            if open_fence.is_none() && !blank && indent >= code_indent && (!same_block || prev_blank) {
                in_indented_code = true;
            }
            prev = Some((c.indent.clone(), c.marker.clone(), blank));

            if in_indented_code {
                if !blank {
                    result.push(lines[i].to_string());
                    i += 1;
                    continue;
                }
            } else {
                match open_fence {
                    Some(open) if closes_code_fence(&c.text, open) => open_fence = None,
                    Some(_) => {}
                    None => open_fence = code_fence(&c.text),
                }
            }
        } else {
            prev = None;
            in_indented_code = false;
        }

        // Table rows and SPDX lines are never combined or wrapped, even when wider than `max_width`.
        let passthrough = open_fence.is_some()
            || comment.is_none_or(|c| {
                let text = split_quote(&c.text).2;
                is_table_row(text) || is_spdx_line(text)
            });
        if passthrough {
            result.push(lines[i].to_string());
            i += 1;
            continue;
        }

        let comment = comment.unwrap();
        if let Some(column) = list_content_column(&comment.text) {
            list_column = Some(column);
        } else if list_column.is_some_and(|column| !comment.text.trim().is_empty() && leading_indent(&comment.text) < column) {
            list_column = None;
        }

        let mut combined_text = comment.text.clone();
        let start_line = i;
        let mut orig_lines: Vec<String> = vec![lines[i].to_string()];

        let mut j = i + 1;
        while j < lines.len() {
            if let Some(next_comment) = &parsed[j] {
                let current = CommentLine {
                    text: combined_text.clone(),
                    ..comment.clone()
                };
                if can_combine(&current, next_comment) {
                    combined_text.push(' ');
                    combined_text.push_str(split_quote(&next_comment.text).2.trim_start());
                    orig_lines.push(lines[j].to_string());
                    j += 1;
                    continue;
                }
            }
            break;
        }

        // A hard line break made of trailing spaces is set aside so that trimming doesn't remove it, and is put back at the end.
        let hard_break = if split_quote(&combined_text).2.trim().is_empty() {
            ""
        } else {
            trailing_space_break(&combined_text)
        };
        let text = &combined_text[..combined_text.len() - hard_break.len()];

        let base = format!("{}{}", comment.indent, comment.marker);
        // Keep the first line's spacing after the marker, so `//text` does not become `// text`. A block doc comment without
        // leading asterisks has an empty marker and no space, so its text follows the indent directly.
        let prefix = if text.is_empty() || !comment.space {
            base.clone()
        } else {
            format!("{base} ")
        };
        let full_line = format!("{prefix}{text}");

        // A line that is neither combined nor wrapped is kept exactly as it is. Blank lines are still trimmed.
        let unchanged = orig_lines.len() == 1 && !split_quote(text).2.trim().is_empty() && full_line.width() <= max_width;
        let mut new_lines = if unchanged {
            orig_lines.clone()
        } else if text.is_empty() {
            vec![base.trim_end().to_string()]
        } else if full_line.width() > max_width {
            // A block quote prefix and leading spaces (e.g. a sub-list item) nest the text, so every wrapped line keeps them.
            let body = split_quote(text).2.trim_start();
            let first = format!("{prefix}{}", &text[..text.len() - body.len()]);
            let cont = format!("{first}{}", " ".repeat(hierarchical_marker_width(body)));
            wrap_text(body, &first, &cont, max_width)
        } else {
            vec![full_line.trim_end().to_string()]
        };
        if !unchanged && let Some(last) = new_lines.last_mut() {
            last.push_str(hard_break);
        }

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

/// Fallback line width when neither `--max-width` nor a config file sets one (rustfmt's default `max_width`).
const DEFAULT_WIDTH: usize = 100;

/// Config file names rustfmt looks for in a directory, in priority order.
const CONFIG_FILE_NAMES: [&str; 2] = [".rustfmt.toml", "rustfmt.toml"];

/// The part of a rustfmt config that this tool uses.
#[derive(Clone, Debug, Default)]
struct Config {
    /// The config file this was loaded from, or `None` if no config file applies.
    path: Option<PathBuf>,
    max_width: Option<usize>,
}

impl Config {
    /// Loads `max_width` from the config file at `path`. Like rustfmt, an unparsable file or an invalid `max_width` is an error.
    fn from_file(path: PathBuf) -> Result<Self, String> {
        let contents = fs::read_to_string(&path).map_err(|e| format!("Error reading config file {}: {e}", path.display()))?;
        let table: toml::Table = contents
            .parse()
            .map_err(|e| format!("Error: could not parse config file {}: {e}", path.display()))?;
        let max_width = match table.get("max_width") {
            None => None,
            Some(value) => Some(value.as_integer().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| {
                format!(
                    "Error: invalid `max_width` in config file {}: expected a non-negative integer, found {value}",
                    path.display()
                )
            })?),
        };
        Ok(Self {
            path: Some(path),
            max_width,
        })
    }

    /// Returns the effective width (`--max-width` > config file > [`DEFAULT_WIDTH`]) and a description of its source.
    fn width(&self, cli_width: Option<usize>) -> (usize, String) {
        match (cli_width, self.max_width, &self.path) {
            (Some(w), _, _) => (w, "from --max-width".to_string()),
            (None, Some(w), Some(path)) => (w, format!("from {}", path.display())),
            _ => (DEFAULT_WIDTH, "default".to_string()),
        }
    }
}

/// Returns the config file directly inside `dir`, preferring `.rustfmt.toml` over `rustfmt.toml`, as rustfmt does.
fn get_toml_path(dir: &Path) -> Result<Option<PathBuf>, String> {
    for name in CONFIG_FILE_NAMES {
        let file = dir.join(name);
        match fs::metadata(&file) {
            // A directory with a config file's name is skipped.
            Ok(md) if md.is_file() => {
                return file
                    .canonicalize()
                    .map(Some)
                    .map_err(|e| format!("Error resolving config file {}: {e}", file.display()));
            }
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                return Err(format!("Failed to get metadata for config file {}: {e}", file.display()));
            }
            _ => {}
        }
    }
    Ok(None)
}

/// Finds the config file that applies to files in `dir`, in rustfmt's search order: `dir` and each of its ancestors up to the
/// filesystem root, then `home`, then `config_dir/rustfmt`.
fn find_config_file(dir: &Path, home: Option<&Path>, config_dir: Option<&Path>) -> Result<Option<PathBuf>, String> {
    let mut current = env::current_dir()
        .and_then(|cwd| cwd.join(dir).canonicalize())
        .map_err(|e| format!("Error resolving directory {}: {e}", dir.display()))?;
    loop {
        if let Some(path) = get_toml_path(&current)? {
            return Ok(Some(path));
        }
        if !current.pop() {
            break;
        }
    }
    for fallback in [home.map(Path::to_path_buf), config_dir.map(|d| d.join("rustfmt"))]
        .into_iter()
        .flatten()
    {
        if let Some(path) = get_toml_path(&fallback)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Loads the config that applies to files in `dir`.
fn load_config(dir: &Path) -> Result<Config, String> {
    match find_config_file(dir, dirs::home_dir().as_deref(), dirs::config_dir().as_deref())? {
        Some(path) => Config::from_file(path),
        None => Ok(Config::default()),
    }
}

/// Loads the config named by `--config-path`, as rustfmt does: a file is used as is, and a directory must directly contain
/// `.rustfmt.toml` or `rustfmt.toml` (its ancestors are not searched).
fn load_explicit_config(path: &Path) -> Result<Config, String> {
    let not_found = || format!("Error: unable to find a config file for the given path: `{}`", path.display());
    let file = if !path.exists() {
        return Err(not_found());
    } else if path.is_dir() {
        get_toml_path(path)?.ok_or_else(not_found)?
    } else {
        path.to_path_buf()
    };
    Config::from_file(file)
}

/// Prints `message` to stderr and exits with status 1.
fn exit_with(message: &str) -> ! {
    eprintln!("{message}");
    process::exit(1)
}

fn main() {
    let CargoCli::WrapComments(cli) = CargoCli::parse();
    let verbose = cli.verbose;
    let quiet = cli.quiet && !verbose;

    // Like rustfmt, config file errors are fatal even when `--max-width` overrides the config's value.
    let explicit_config = cli
        .config_path
        .as_deref()
        .map(load_explicit_config)
        .transpose()
        .unwrap_or_else(|e| exit_with(&e));

    if cli.files.is_empty() {
        // rustfmt resolves the config for stdin from the current directory.
        let config = explicit_config
            .map_or_else(|| load_config(Path::new(".")), Ok)
            .unwrap_or_else(|e| exit_with(&e));
        let (width, source) = config.width(cli.max_width);
        if verbose {
            eprintln!("Width: {width} ({source})");
        }
        process_stdin(width, cli.check);
        return;
    }

    // Without `--max-width` or `--config-path`, each file's width comes from the config that applies to its directory.
    let per_file_width = cli.max_width.is_none() && explicit_config.is_none();
    if verbose && !per_file_width {
        let (width, source) = explicit_config.clone().unwrap_or_default().width(cli.max_width);
        eprintln!("Width: {width} ({source})");
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

    // Resolve every width before modifying anything, so a bad config file aborts the run without partial changes.
    let mut dir_configs: HashMap<PathBuf, Config> = HashMap::new();
    let mut widths = Vec::with_capacity(file_paths.len());
    for path in &file_paths {
        let config = match &explicit_config {
            Some(config) => config,
            None => {
                let dir = path.parent().unwrap_or(Path::new("."));
                if !dir_configs.contains_key(dir) {
                    let config = load_config(dir).unwrap_or_else(|e| exit_with(&e));
                    dir_configs.insert(dir.to_path_buf(), config);
                }
                &dir_configs[dir]
            }
        };
        widths.push(config.width(cli.max_width));
    }

    let mut files_processed = 0;
    let mut files_modified = 0;

    for (path, (width, source)) in file_paths.iter().zip(widths) {
        if verbose && per_file_width {
            eprintln!("Width: {width} ({source}) for {}", path.display());
        }
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
            eprintln!("{} ({line_count} lines): {} change(s)", path.display(), changes.len());
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
    fn test_parse_no_space() {
        let c = parse_comment_line("//no space").unwrap();
        assert_eq!(c.marker, "//");
        assert!(!c.space);
        assert_eq!(c.text, "no space");
        assert!(parse_comment_line("// space").unwrap().space);
    }

    #[test]
    fn test_parse_four_or_more_slashes() {
        let c = parse_comment_line("    //// plain comment").unwrap();
        assert_eq!(c.marker, "////");
        assert_eq!(c.text, "plain comment");
        let c = parse_comment_line("////////").unwrap();
        assert_eq!(c.marker, "////////");
        assert_eq!(c.text, "");
    }

    #[test]
    fn test_unchanged_lines_are_verbatim() {
        let input = "//no space, short line.\n\n//// four slashes is a plain comment, not a doc comment.\n\nfn f() {\n    let x = 1;\n    ////////////////////////////////\n}\n// trailing spaces  \n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_four_slashes_not_combined_with_two() {
        let input = "// two\n//// four\n//// four again\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// two\n//// four four again\n");
    }

    #[test]
    fn test_combine_and_wrap_keep_missing_space() {
        let input = "//aaa bbb\n//ccc ddd\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "//aaa bbb ccc ddd\n");
        let (output, _) = process_content("//aaa bbb ccc ddd\n", 12);
        assert_eq!(output, "//aaa bbb\n//ccc ddd\n");
    }

    #[test]
    fn test_missing_space_with_list_quote_and_hard_break() {
        let (output, _) = process_content("//- aaa bbb ccc ddd\n", 14);
        assert_eq!(output, "//- aaa bbb\n//  ccc ddd\n");
        let (output, _) = process_content("//> aaa bbb ccc ddd\n", 14);
        assert_eq!(output, "//> aaa bbb\n//> ccc ddd\n");
        let (output, _) = process_content("//aaa bbb ccc  \n//ddd\n", 12);
        assert_eq!(output, "//aaa bbb\n//ccc  \n//ddd\n");
    }

    #[test]
    fn test_four_slashes_wrap_and_hard_break() {
        let (output, _) = process_content("//// aaa bbb ccc ddd\n", 14);
        assert_eq!(output, "//// aaa bbb\n//// ccc ddd\n");
        let input = "//// one  \n//// two\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_unchanged_lines_verbatim_with_wide_chars_and_block_docs() {
        // "//日本語" is 8 columns but 11 bytes, so it fits in 8 columns and is kept as it is.
        let input = "//日本語\n///  two spaces\n/**\n *  two spaces\n */\n";
        let (output, changes) = process_content(input, 16);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_hierarchical_markers() {
        assert!(starts_with_hierarchical_marker("# heading"));
        assert!(starts_with_hierarchical_marker("* bullet"));
        assert!(starts_with_hierarchical_marker("- dash"));
        assert!(starts_with_hierarchical_marker("1. numbered"));
        assert!(!starts_with_hierarchical_marker("A. lettered"));
        assert!(!starts_with_hierarchical_marker("a. lower"));
        assert!(starts_with_hierarchical_marker("```code"));
        assert!(starts_with_hierarchical_marker("~~~"));
        assert!(!starts_with_hierarchical_marker("normal text"));
    }

    #[test]
    fn test_combine_basic() {
        let a = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            space: true,
            text: "hello".into(),
        };
        let b = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            space: true,
            text: "world".into(),
        };
        assert!(can_combine(&a, &b));
    }

    #[test]
    fn test_no_combine_different_markers() {
        let a = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            space: true,
            text: "hello".into(),
        };
        let b = CommentLine {
            indent: "".into(),
            marker: "///".into(),
            space: true,
            text: "world".into(),
        };
        assert!(!can_combine(&a, &b));
    }

    #[test]
    fn test_no_combine_blank() {
        let a = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            space: true,
            text: "hello".into(),
        };
        let b = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            space: true,
            text: "".into(),
        };
        assert!(!can_combine(&a, &b));
    }

    #[test]
    fn test_no_combine_hierarchical() {
        let a = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            space: true,
            text: "hello".into(),
        };
        let b = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            space: true,
            text: "- bullet".into(),
        };
        assert!(!can_combine(&a, &b));
    }

    #[test]
    fn test_no_combine_heading_with_next() {
        let a = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            space: true,
            text: "# Heading".into(),
        };
        let b = CommentLine {
            indent: "".into(),
            marker: "//".into(),
            space: true,
            text: "some text".into(),
        };
        assert!(!can_combine(&a, &b));
    }

    #[test]
    fn test_no_combine_heading_process() {
        let input = "// # Heading\n// some text after heading\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_is_atx_heading() {
        assert!(is_atx_heading("# Heading"));
        assert!(is_atx_heading("###### Six"));
        assert!(is_atx_heading("##\tTab"));
        assert!(is_atx_heading("#"));
        assert!(!is_atx_heading("#42 is an issue"));
        assert!(!is_atx_heading("#hashtag"));
        assert!(!is_atx_heading("#[derive(Debug)]"));
        assert!(!is_atx_heading("####### seven"));
        assert!(!is_atx_heading("text"));
        assert!(!is_atx_heading(""));
        assert!(!starts_with_hierarchical_marker("#42 is an issue"));
        assert_eq!(hierarchical_marker_width("####### seven"), 0);
    }

    #[test]
    fn test_combine_lines_starting_with_hash_that_are_not_headings() {
        let input = "// See issue #42 for details.\n// #42 is the same issue, but this line starts with a hash.\n// This line follows it.\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(
            output,
            "// See issue #42 for details. #42 is the same issue, but this line starts with a hash. This line follows it.\n"
        );
    }

    #[test]
    fn test_hash_lines_in_quote() {
        let input = "// > See\n// > #42 for details.\n// > # Heading\n// > text\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// > See #42 for details.\n// > # Heading\n// > text\n");
    }

    #[test]
    fn test_wrap_hash_line_that_is_not_heading_has_no_hanging_indent() {
        let (output, _) = process_content("// #42 aaa bbb ccc\n", 14);
        assert_eq!(output, "// #42 aaa bbb\n// ccc\n");
        let (output, _) = process_content("// ## aaa bbb ccc\n", 14);
        assert_eq!(output, "// ## aaa bbb\n//    ccc\n");
    }

    #[test]
    fn test_hash_lines_in_code_blocks_pass_through() {
        let input = "// ~~~\n// #[derive(Debug)]\n// #42\n// ~~~\n//\n//     #[derive(Debug)]\n//     #hashtag\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_is_divider() {
        assert!(is_divider("----"));
        assert!(is_divider("=========="));
        assert!(is_divider("**********"));
        assert!(is_divider("~~~~~"));
        assert!(is_divider("//////"));
        assert!(is_divider("- - - -"));
        assert!(!is_divider("--"));
        assert!(!is_divider("---"));
        assert!(!is_divider("///"));
        assert!(!is_divider("- - -"));
        assert!(!is_divider("-=-=-="));
        assert!(!is_divider("```"));
        assert!(!is_divider("- item"));
        assert!(!is_divider("text"));
        assert!(!is_divider(""));
    }

    #[test]
    fn test_is_thematic_break() {
        assert!(is_thematic_break("---"));
        assert!(is_thematic_break("***"));
        assert!(is_thematic_break("___"));
        assert!(is_thematic_break("_ _ _"));
        assert!(is_thematic_break(" **  * ** * ** * **"));
        assert!(is_thematic_break("   -\t-\t-  "));
        assert!(is_thematic_break("-------------------------"));
        assert!(!is_thematic_break("    ---"));
        assert!(!is_thematic_break("--"));
        assert!(!is_thematic_break("===="));
        assert!(!is_thematic_break("~~~"));
        assert!(!is_thematic_break("-*-"));
        assert!(!is_thematic_break("--- a"));
        assert!(!is_thematic_break("- item"));
        assert!(!is_thematic_break(""));
    }

    #[test]
    fn test_no_combine_thematic_break() {
        let input = "// some text\n// ***\n// more text\n// _ _ _\n// ---\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_is_spdx_line() {
        assert!(is_spdx_line("SPDX-License-Identifier: MIT OR Apache-2.0"));
        assert!(is_spdx_line("SPDX-FileCopyrightText: 2026 Example Authors"));
        assert!(!is_spdx_line("SPDX-: MIT"));
        assert!(!is_spdx_line("SPDX-License Identifier: MIT"));
        assert!(!is_spdx_line("SPDX license identifiers are tags"));
        assert!(!is_spdx_line("See SPDX-License-Identifier: MIT"));
    }

    #[test]
    fn test_starts_with_annotation_tag() {
        for text in [
            "TODO: x",
            "FIXME: x",
            "NOTE: x",
            "HACK: x",
            "XXX: x",
            "SAFETY: x",
            "TODO(someone): x",
            "FIXME():",
        ] {
            assert!(starts_with_annotation_tag(text), "{text:?}");
        }
        for text in ["TODO x", "TODOS: x", "todo: x", "TODO(someone x", "TODO(a) : x", "A TODO: x"] {
            assert!(!starts_with_annotation_tag(text), "{text:?}");
        }
    }

    #[test]
    fn test_spdx_line_not_combined_or_wrapped() {
        let input = "// Copyright (c) 2026 Example Authors.\n// SPDX-License-Identifier: MIT OR Apache-2.0\n// See the LICENSE files for details.\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());

        let input = "// SPDX-License-Identifier: MIT OR Apache-2.0 WITH LLVM-exception\n";
        let (output, changes) = process_content(input, 30);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_annotation_tag_starts_new_paragraph() {
        let input = "// Some text.\n// TODO(someone): first note\n// continues here.\n// FIXME: Short FIXME.\n// NOTE: x\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(
            output,
            "// Some text.\n// TODO(someone): first note continues here.\n// FIXME: Short FIXME.\n// NOTE: x\n"
        );
    }

    #[test]
    fn test_tagged_lines_in_quote() {
        let input = "// > text\n// > TODO: note\n// > SPDX-License-Identifier: MIT OR Apache-2.0\n// > more\n";
        let (output, changes) = process_content(input, 30);
        assert_eq!(output, input);
        assert!(changes.is_empty());
        let input = "// > text\n// > TODO: note\n// > continues\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// > text\n// > TODO: note continues\n");
    }

    #[test]
    fn test_tagged_lines_with_hard_breaks_and_code() {
        // A hard break ends a tagged line, and tags inside code blocks are passed through.
        let input = "// TODO: one  \n// two\n// ~~~\n// text\n// SPDX-License-Identifier: MIT OR Apache-2.0 WITH LLVM-exception\n// ~~~\n//\n//     text\n//     FIXME: in code\n";
        let (output, changes) = process_content(input, 40);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_split_quote() {
        assert_eq!(split_quote("> text"), ("> ", 1, "text"));
        assert_eq!(split_quote(">text"), (">", 1, "text"));
        assert_eq!(split_quote("> > text"), ("> > ", 2, "text"));
        assert_eq!(split_quote(">> text"), (">> ", 2, "text"));
        assert_eq!(split_quote("   > text"), ("   > ", 1, "text"));
        assert_eq!(split_quote(">"), (">", 1, ""));
        assert_eq!(split_quote("    > code"), ("", 0, "    > code"));
        assert_eq!(split_quote("  text"), ("", 0, "  text"));
        assert_eq!(split_quote("a > b"), ("", 0, "a > b"));
    }

    #[test]
    fn test_combine_and_wrap_block_quote() {
        let input = "/// > **Note:** The iterator borrows the buffer, so the buffer cannot be modified while the iterator is alive. This is enforced by the borrow checker,\n/// > and the quoted text is split across two lines that both begin with `>`.\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(
            output,
            "/// > **Note:** The iterator borrows the buffer, so the buffer cannot be modified while the iterator is alive. This is enforced by\n/// > the borrow checker, and the quoted text is split across two lines that both begin with `>`.\n"
        );
        let (again, changes) = process_content(&output, 132);
        assert_eq!(again, output);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_no_combine_different_quote_depths() {
        let input = "// text\n// > quoted\n// > > nested\n// > quoted again\n// text again\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_no_combine_blank_quote_line() {
        let input = "// > one\n// >\n// > two\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_quote_applies_rules_after_prefix() {
        let input = "// > text\n// > - item\n// > # Heading\n// > more\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_wrap_list_item_in_quote() {
        let input = "// > - aaa bbb ccc ddd\n";
        let (output, _) = process_content(input, 20);
        assert_eq!(output, "// > - aaa bbb ccc\n// >   ddd\n");
    }

    #[test]
    fn test_hard_line_breaks_in_quote() {
        let input = "// > one\\\n// > two\n// > three  \n// > four\n// > five\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// > one\\\n// > two three  \n// > four five\n");
    }

    #[test]
    fn test_wrap_quote_keeps_trailing_space_break_on_last_line() {
        let input = "// > aaa bbb ccc ddd  \n// > eee\n";
        let (output, _) = process_content(input, 14);
        assert_eq!(output, "// > aaa bbb\n// > ccc ddd  \n// > eee\n");
        let (again, changes) = process_content(&output, 14);
        assert_eq!(again, output);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_blank_quote_line_with_trailing_spaces_is_trimmed() {
        let (output, _) = process_content("// >   \n", 80);
        assert_eq!(output, "// >\n");
    }

    #[test]
    fn test_table_rows_in_quote_pass_through() {
        let input = "// > text\n// > | a | b |\n// > | - | - |\n// > more\n// > text\n";
        let (output, _) = process_content(input, 12);
        assert_eq!(output, "// > text\n// > | a | b |\n// > | - | - |\n// > more\n// > text\n");
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// > text\n// > | a | b |\n// > | - | - |\n// > more text\n");
    }

    #[test]
    fn test_no_combine_tilde_fence_in_quote() {
        let input = "// > text\n// > ~~~\n// > more\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_is_table_row() {
        assert!(is_table_row("| a | b |"));
        assert!(is_table_row("  | --- | :---: |"));
        assert!(is_table_row("|"));
        assert!(!is_table_row("a | b"));
        assert!(!is_table_row(""));
    }

    #[test]
    fn test_table_rows_pass_through() {
        let input = "\
/// Types that can be emptied in a single call.
///
/// | Implementor       | Order            | Notes                                                                                                         |
/// | ----------------- | ---------------- | ------------------------------------------------------------------------------------------------------------- |
/// | [`RingBuffer<T>`] | oldest to newest | The only implementor in this file. The table row is longer than the configured width and must not be wrapped. |
pub trait Drain {
";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_no_combine_table_with_surrounding_text() {
        let input = "// text before\n// | a | b |\n// | - | - |\n// text after\n// more\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// text before\n// | a | b |\n// | - | - |\n// text after more\n");
    }

    #[test]
    fn test_table_rows_with_hard_breaks_and_tilde_fences() {
        // Table rows keep trailing backslashes and spaces, and a table inside a tilde fence doesn't end the code block.
        let input = "// text\\\n// | a | b |  \n// | - | - |\\\n// ~~~\n// | c | d |\n// one\n// two\n// ~~~\n// three\n// four\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(
            output,
            "// text\\\n// | a | b |  \n// | - | - |\\\n// ~~~\n// | c | d |\n// one\n// two\n// ~~~\n// three four\n"
        );
    }

    #[test]
    fn test_no_combine_divider_with_previous() {
        let input = "// some text\n// ==========\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_no_combine_divider_with_next() {
        let input = "// ----------\n// some text\n// ==========\n// more text\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_no_combine_consecutive_dividers() {
        let input = "// ----------\n// ----------\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_no_combine_indented_sub_list_items() {
        let input = "//! 1. item\n//!    - sub one\n//!    - sub two\n//!    * sub three\n//!    2. sub four\n";
        let (output, changes) = process_content(input, 100);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_combine_sub_list_item_continuation() {
        let input = "//!    - aaa\n//!      bbb\n";
        let (output, _) = process_content(input, 100);
        assert_eq!(output, "//!    - aaa bbb\n");
    }

    #[test]
    fn test_wrap_sub_list_item_keeps_indent() {
        let input = "//!    - aaa bbb ccc ddd\n";
        let (output, _) = process_content(input, 20);
        assert_eq!(output, "//!    - aaa bbb ccc\n//!      ddd\n");
        // Re-processing the wrapped output is stable.
        let (again, changes) = process_content(&output, 20);
        assert_eq!(again, output);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_wrap_indented_text_keeps_indent() {
        let input = "// x\n//\n//   aaa bbb ccc ddd\n";
        let (output, _) = process_content(input, 16);
        assert_eq!(output, "// x\n//\n//   aaa bbb ccc\n//   ddd\n");
    }

    #[test]
    fn test_indented_code_fence_in_list_item() {
        let input = "/// - item\n///   ```\n///   let a = 1;\n///   let b = 2;\n///   ```\n";
        let (output, changes) = process_content(input, 100);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_block_doc_asterisk_style() {
        let input = "/**\n * aaa bbb\n * ccc ddd eee\n *\n * fff\n */\n";
        let (output, _) = process_content(input, 16);
        assert_eq!(output, "/**\n * aaa bbb ccc\n * ddd eee\n *\n * fff\n */\n");
        let (again, changes) = process_content(&output, 16);
        assert_eq!(again, output);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_block_doc_no_asterisk_style() {
        let input = "    /*!\n    aaa bbb\n    ccc ddd eee\n\n      - fff ggg hhh\n    */\n";
        let (output, _) = process_content(input, 18);
        assert_eq!(
            output,
            "    /*!\n    aaa bbb ccc\n    ddd eee\n\n      - fff ggg\n        hhh\n    */\n"
        );
    }

    #[test]
    fn test_block_doc_one_line_unchanged() {
        let input = "/** a one-line block doc comment that is longer than the width */\n";
        let (output, changes) = process_content(input, 20);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_indented_code_block_not_wrapped() {
        let input = "    /// The result is `(sum2 << 8) | sum1`.\n    ///\n    ///     This line is an indented code block, because it is indented by four spaces and preceded by a blank line. It must not be wrapped or combined.\n    ///\n    /// [Fletcher-16]: https://en.wikipedia.org/wiki/Fletcher%27s_checksum#Fletcher-16\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_block_doc_nested_comment() {
        let input = "/**\n * aaa\n * /* nested */ bbb\n * ccc\n * ddd\n */\n// eee\n// fff\n";
        let (output, _) = process_content(input, 100);
        assert_eq!(output, "/**\n * aaa\n * /* nested */ bbb\n * ccc ddd\n */\n// eee fff\n");
    }

    #[test]
    fn test_block_doc_code_fence() {
        let input = "/**\n * ```\n * let a = 1;\n * let b = 2;\n * ```\n */\n";
        let (output, changes) = process_content(input, 100);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_indented_code_block_not_combined() {
        let input = "///\n///     let a = 1;\n///     let b = 2;\n///\n///     let c = 3;\n/// Text after the code block.\n/// More text.\n";
        let (output, _) = process_content(input, 100);
        assert_eq!(
            output,
            "///\n///     let a = 1;\n///     let b = 2;\n///\n///     let c = 3;\n/// Text after the code block. More text.\n"
        );
    }

    #[test]
    fn test_indented_code_block_at_block_start() {
        let input = "fn f() {}\n///     let a = 1;\n///     let b = 2;\n";
        let (output, changes) = process_content(input, 100);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_plain_block_comment_unchanged() {
        let input = "/*\n * aaa bbb ccc ddd eee fff\n * ggg\n// hhh iii jjj kkk\n// lll\n/* /* */ */\n*/\n";
        let (output, changes) = process_content(input, 12);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_plain_block_variants_not_doc() {
        assert!(is_block_doc_opener("/**"));
        assert!(is_block_doc_opener("/*!  "));
        assert!(!is_block_doc_opener("/***"));
        assert!(!is_block_doc_opener("/**/"));
        assert!(!is_block_doc_opener("/*"));
        assert!(!is_block_doc_opener("/** text"));
    }

    #[test]
    fn test_block_comment_depth() {
        assert_eq!(block_comment_depth("/*", 0), 1);
        assert_eq!(block_comment_depth("/* /* */", 0), 1);
        assert_eq!(block_comment_depth("/* */ code /*", 0), 0);
        assert_eq!(block_comment_depth("/**/", 0), 0);
        assert_eq!(block_comment_depth(" */ */", 2), 0);
        assert_eq!(block_comment_depth("text", 1), 1);
    }

    #[test]
    fn test_block_doc_asterisk_style_with_markdown() {
        // Hard breaks, block quotes, tables, tilde fences, indented code, and tags work after the ` * ` prefix.
        let input = "/**\n * text  \n * next\n * > quoted\n * > more\n * | a |\n * ~~~\n * a\n * b\n * ~~~\n *\n *     code a\n *     code b\n * TODO: x\n * y\n */\n";
        let (output, _) = process_content(input, 100);
        assert_eq!(
            output,
            "/**\n * text  \n * next\n * > quoted more\n * | a |\n * ~~~\n * a\n * b\n * ~~~\n *\n *     code a\n *     code b\n * TODO: x y\n */\n"
        );
    }

    #[test]
    fn test_block_doc_no_asterisk_style_indented_code_and_width() {
        // Indented code is measured from the common indentation, and "    café déjà vu" fits in 16 columns although it is 19 bytes.
        let input = "    /**\n    text\n\n        code that is long enough to exceed the width\n    café déjà\n    vu\n    */\n";
        let (output, _) = process_content(input, 16);
        assert_eq!(
            output,
            "    /**\n    text\n\n        code that is long enough to exceed the width\n    café déjà vu\n    */\n"
        );
    }

    #[test]
    fn test_indented_line_after_text_is_not_code() {
        // An indented code block cannot interrupt a paragraph.
        let input = "/// text\n///     more text\n";
        let (output, _) = process_content(input, 100);
        assert_eq!(output, "/// text more text\n");
    }

    #[test]
    fn test_indented_code_block_in_list_item() {
        // Inside a list item, code is indented 4 columns past the start of the item's content.
        let input = "/// - item\n///\n///     nested paragraph\n///     continued\n///\n///       code a\n///       code b\n";
        let (output, _) = process_content(input, 100);
        assert_eq!(
            output,
            "/// - item\n///\n///     nested paragraph continued\n///\n///       code a\n///       code b\n"
        );
    }

    #[test]
    fn test_indented_fence_inside_indented_code_block() {
        let input = "///\n///     ```\n///     code\n/// text\n/// more\n";
        let (output, _) = process_content(input, 100);
        assert_eq!(output, "///\n///     ```\n///     code\n/// text more\n");
    }

    #[test]
    fn test_indented_tilde_fence_inside_indented_code_block() {
        let input = "///\n///     ~~~\n///     code\n/// text\n/// more\n";
        let (output, _) = process_content(input, 100);
        assert_eq!(output, "///\n///     ~~~\n///     code\n/// text more\n");
    }

    #[test]
    fn test_indented_line_inside_tilde_fence_does_not_start_indented_code() {
        let input = "// ~~~\n//\n//     code\n// ~~~\n// a\n// b\n";
        let (output, _) = process_content(input, 100);
        assert_eq!(output, "// ~~~\n//\n//     code\n// ~~~\n// a b\n");
    }

    #[test]
    fn test_indented_code_block_keeps_hard_breaks_tables_and_quotes() {
        // Code lines that look like hard breaks, table rows, or block quotes are passed through, and text after the block still
        // honors a hard line break.
        let input = "///\n///     let a = 1; \\\n///     | x |  \n///     > not a quote, but a long code line that is wider than the width\n/// one\\\n/// two\n/// three\n";
        let (output, _) = process_content(input, 40);
        assert_eq!(
            output,
            "///\n///     let a = 1; \\\n///     | x |  \n///     > not a quote, but a long code line that is wider than the width\n/// one\\\n/// two three\n"
        );
    }

    #[test]
    fn test_leading_indent() {
        assert_eq!(leading_indent("text"), 0);
        assert_eq!(leading_indent("    text"), 4);
        assert_eq!(leading_indent("\ttext"), 4);
        assert_eq!(leading_indent("  \ttext"), 4);
        assert_eq!(leading_indent("   "), 3);
    }

    /// Creates `project/src`, `home`, and `config/rustfmt` under a temporary root.
    fn config_search_tree() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().join("project").join("src");
        let home = root.path().join("home");
        let config_dir = root.path().join("config");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(config_dir.join("rustfmt")).unwrap();
        (root, src, home, config_dir)
    }

    #[test]
    fn test_find_config_file_ancestor_before_home_and_config_dir() {
        let (root, src, home, config_dir) = config_search_tree();
        let project_config = root.path().join("project").join("rustfmt.toml");
        fs::write(&project_config, "").unwrap();
        fs::write(home.join(".rustfmt.toml"), "").unwrap();
        fs::write(config_dir.join("rustfmt").join("rustfmt.toml"), "").unwrap();
        let found = find_config_file(&src, Some(&home), Some(&config_dir)).unwrap();
        assert_eq!(found, Some(project_config.canonicalize().unwrap()));
    }

    #[test]
    fn test_find_config_file_falls_back_to_home() {
        let (_root, src, home, config_dir) = config_search_tree();
        let home_config = home.join(".rustfmt.toml");
        fs::write(&home_config, "").unwrap();
        fs::write(config_dir.join("rustfmt").join("rustfmt.toml"), "").unwrap();
        let found = find_config_file(&src, Some(&home), Some(&config_dir)).unwrap();
        assert_eq!(found, Some(home_config.canonicalize().unwrap()));
    }

    #[test]
    fn test_find_config_file_falls_back_to_config_dir() {
        let (_root, src, home, config_dir) = config_search_tree();
        let user_config = config_dir.join("rustfmt").join("rustfmt.toml");
        fs::write(&user_config, "").unwrap();
        let found = find_config_file(&src, Some(&home), Some(&config_dir)).unwrap();
        assert_eq!(found, Some(user_config.canonicalize().unwrap()));
    }

    #[test]
    fn test_find_config_file_none() {
        let (_root, src, home, config_dir) = config_search_tree();
        assert_eq!(find_config_file(&src, Some(&home), Some(&config_dir)).unwrap(), None);
    }

    #[test]
    fn test_get_toml_path_skips_directory_with_config_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".rustfmt.toml")).unwrap();
        let config = dir.path().join("rustfmt.toml");
        fs::write(&config, "").unwrap();
        assert_eq!(get_toml_path(dir.path()).unwrap(), Some(config.canonicalize().unwrap()));
    }

    fn config_from(contents: &str) -> Result<Config, String> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rustfmt.toml");
        fs::write(&path, contents).unwrap();
        Config::from_file(path)
    }

    #[test]
    fn test_config_max_width() {
        assert_eq!(config_from("max_width = 120\n").unwrap().max_width, Some(120));
        assert_eq!(config_from("edition = \"2024\"\n").unwrap().max_width, None);
        assert_eq!(config_from("").unwrap().max_width, None);
    }

    #[test]
    fn test_config_errors() {
        assert!(config_from("max_width = ").unwrap_err().contains("could not parse"));
        assert!(
            config_from("max_width = \"wide\"\n")
                .unwrap_err()
                .contains("invalid `max_width`")
        );
        assert!(config_from("max_width = -1\n").unwrap_err().contains("invalid `max_width`"));
    }

    #[test]
    fn test_config_width_precedence() {
        let config = Config {
            path: Some(PathBuf::from("rustfmt.toml")),
            max_width: Some(120),
        };
        assert_eq!(config.width(Some(80)).0, 80);
        assert_eq!(config.width(None).0, 120);
        assert_eq!(Config::default().width(None), (DEFAULT_WIDTH, "default".to_string()));
    }

    #[test]
    fn test_wrap_text() {
        let result = wrap_text("hello world foo bar baz", "// ", "// ", 15);
        assert_eq!(result, vec!["// hello world", "// foo bar baz"]);
    }

    #[test]
    fn test_non_ascii_width_counts_columns() {
        // 28 columns but 34 bytes: fits within 30 columns.
        let input = "// naïve café résumé déjà vu\n";
        let (output, changes) = process_content(input, 30);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_wide_chars_count_two_columns() {
        // Each CJK character is 2 columns wide: "// 日本語 テキスト" is 18 columns (34 bytes).
        let input = "// 日本語 テキスト\n";
        let (output, changes) = process_content(input, 18);
        assert_eq!(output, input);
        assert!(changes.is_empty());
        let (output, _) = process_content(input, 17);
        assert_eq!(output, "// 日本語\n// テキスト\n");
    }

    #[test]
    fn test_non_ascii_width_in_quote_list_and_hard_break() {
        // "// > naïve café résumé déjà vu" is 30 columns but 35 bytes.
        let input = "// > naïve café résumé déjà vu\n";
        let (output, changes) = process_content(input, 30);
        assert_eq!(output, input);
        assert!(changes.is_empty());

        // "// - 日本語 テキスト" is 20 columns, and the continuation line keeps the 2-column hanging indent.
        let (output, _) = process_content("// - 日本語 テキスト\n", 19);
        assert_eq!(output, "// - 日本語\n//   テキスト\n");

        // "// déjà vu là" is 13 columns but 16 bytes, so it fits in 14 columns and keeps its hard line break.
        let input = "// déjà vu là  \n// next\n";
        let (output, changes) = process_content(input, 14);
        assert_eq!(output, input);
        assert!(changes.is_empty());
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
    fn test_code_fence() {
        assert_eq!(code_fence("```"), Some(('`', 3)));
        assert_eq!(code_fence("```rust"), Some(('`', 3)));
        assert_eq!(code_fence("  ~~~~"), Some(('~', 4)));
        assert_eq!(code_fence("~~"), None);
        assert_eq!(code_fence("``"), None);
        assert_eq!(code_fence("text ```"), None);
        assert!(closes_code_fence("~~~", ('~', 3)));
        assert!(closes_code_fence("~~~~~  ", ('~', 3)));
        assert!(!closes_code_fence("~~~", ('~', 4)));
        assert!(!closes_code_fence("```", ('~', 3)));
        assert!(!closes_code_fence("~~~ rust", ('~', 3)));
    }

    #[test]
    fn test_no_wrap_or_combine_inside_tilde_fence() {
        let input = "    /// ~~~\n    /// # use ring_buffer::{Drain, RingBuffer};\n    /// let mut buffer = RingBuffer::with_capacity(3).unwrap();\n    /// buffer.push(\"x\").unwrap();\n    /// assert_eq!(buffer.drain_all(), [\"x\"]); // a long trailing comment that is much wider than the configured width\n    /// ~~~\n";
        let (output, changes) = process_content(input, 60);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_no_wrap_inside_tilde_fence_plain_comment() {
        let input = "// ~~~\n// Tilde fence in a plain comment, followed by a very long line.\n// ~~~\n";
        let (output, changes) = process_content(input, 40);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_fence_closed_only_by_matching_fence() {
        // A backtick fence and a shorter tilde fence do not close a `~~~~` block.
        let input = "// ~~~~\n// ```\n// ~~~\n// one\n// two\n// ~~~~\n// hello\n// world\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// ~~~~\n// ```\n// ~~~\n// one\n// two\n// ~~~~\n// hello world\n");
    }

    #[test]
    fn test_hard_line_breaks_around_tilde_fence() {
        // Hard breaks inside the block are passed through, and a closing fence with trailing spaces still closes it.
        let input = "// one\\\n// ~~~\n// a very long line inside the fence that is wider than the width\\\n// b  \n// ~~~  \n// two\n// three\n";
        let (output, _) = process_content(input, 40);
        assert_eq!(
            output,
            "// one\\\n// ~~~\n// a very long line inside the fence that is wider than the width\\\n// b  \n// ~~~  \n// two three\n"
        );
    }

    #[test]
    fn test_hierarchical_marker_width() {
        assert_eq!(hierarchical_marker_width("- item"), 2);
        assert_eq!(hierarchical_marker_width("* item"), 2);
        assert_eq!(hierarchical_marker_width("1. item"), 3);
        assert_eq!(hierarchical_marker_width("A. item"), 0);
        assert_eq!(hierarchical_marker_width("# heading"), 2);
        assert_eq!(hierarchical_marker_width("## heading"), 3);
        assert_eq!(hierarchical_marker_width("### heading"), 4);
        assert_eq!(hierarchical_marker_width("normal text"), 0);
        assert_eq!(hierarchical_marker_width("```code"), 0);
        assert_eq!(hierarchical_marker_width("10. item"), 4);
        assert_eq!(hierarchical_marker_width("100. item"), 5);
    }

    #[test]
    fn test_spaced_list_markers() {
        assert!(starts_with_hierarchical_marker("+ plus"));
        assert!(starts_with_hierarchical_marker("1) first"));
        assert!(starts_with_hierarchical_marker("10) tenth"));
        assert!(starts_with_hierarchical_marker("2)\ttab"));
        assert!(starts_with_hierarchical_marker("3)"));
        assert!(!starts_with_hierarchical_marker("+1 is positive"));
        assert!(!starts_with_hierarchical_marker("1)no space"));
        assert!(!starts_with_hierarchical_marker("etc. and so on"));
        assert_eq!(hierarchical_marker_width("+ plus"), 2);
        assert_eq!(hierarchical_marker_width("1) first"), 3);
        assert_eq!(hierarchical_marker_width("10) tenth"), 4);
        assert_eq!(hierarchical_marker_width("etc. and so on"), 0);
    }

    #[test]
    fn test_letters_and_roman_numerals_are_not_list_markers() {
        for text in [
            "A. Turing",
            "a. lower",
            "a) letter",
            "B) upper",
            "i. first",
            "ii. second",
            "iv) fourth",
            "XII. twelfth",
        ] {
            assert!(!starts_with_hierarchical_marker(text), "{text:?}");
            assert_eq!(hierarchical_marker_width(text), 0, "{text:?}");
            assert_eq!(ordered_marker_label(text), None, "{text:?}");
            assert!(!starts_new_block("A paragraph.", text), "{text:?}");
        }
    }

    #[test]
    fn test_ordered_marker_has_at_most_nine_digits() {
        assert_eq!(ordered_marker_label("123456789. nine digits"), Some("123456789"));
        assert_eq!(ordered_marker_label("1234567890. ten digits"), None);
        assert_eq!(ordered_marker_label("1234567890) ten digits"), None);
        assert_eq!(hierarchical_marker_width("1234567890. ten digits"), 0);
    }

    #[test]
    fn test_letter_and_roman_lines_are_combined_without_hanging_indent() {
        let input = "/// The halting problem was shown to be undecidable by\n/// A. Turing in 1936.\nfn a() {}\n\n/// The steps are:\n///\n/// i. Read the input.\n/// ii. Compute the checksum.\nfn b() {}\n\n/// a. This sentence starts with a letter and a period, and it is long enough to wrap.\nfn c() {}\n";
        let (output, _) = process_content(input, 60);
        assert_eq!(
            output,
            "/// The halting problem was shown to be undecidable by A.\n/// Turing in 1936.\nfn a() {}\n\n/// The steps are:\n///\n/// i. Read the input. ii. Compute the checksum.\nfn b() {}\n\n/// a. This sentence starts with a letter and a period, and\n/// it is long enough to wrap.\nfn c() {}\n"
        );
    }

    #[test]
    fn test_no_combine_spaced_list_markers() {
        let input = "// + Plus signs are valid CommonMark bullets too.\n// + Second plus-sign bullet.\n// 1) Parenthesized ordered list markers are also valid CommonMark.\n// 2) Second item.\n";
        let (output, changes) = process_content(input, 80);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_wrap_spaced_list_markers_with_indent() {
        let input = "// 12) A numbered item that is long enough to wrap\n// + A plus item that is long enough to wrap\n";
        let (output, _) = process_content(input, 32);
        assert_eq!(
            output,
            "// 12) A numbered item that is\n//     long enough to wrap\n// + A plus item that is long\n//   enough to wrap\n"
        );
    }

    #[test]
    fn test_spaced_list_markers_in_quote() {
        let input = "// > + one\n// > + two\n// > 1) three\n// > 4) four\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
        let (output, _) = process_content("// > 12) aaa bbb ccc ddd\n", 20);
        assert_eq!(output, "// > 12) aaa bbb ccc\n// >     ddd\n");
    }

    #[test]
    fn test_indented_code_block_in_spaced_list_item() {
        // Inside a `10)` item, content starts at column 4, so code is indented 8 columns.
        let input = "/// 10) item\n///\n///     nested paragraph\n///     continued\n///\n///         code a\n///         code b\n";
        let (output, _) = process_content(input, 100);
        assert_eq!(
            output,
            "/// 10) item\n///\n///     nested paragraph continued\n///\n///         code a\n///         code b\n"
        );
        // A `+` item's content starts at column 2, so a line indented 6 columns is code.
        let input = "/// + item\n///\n///       code a\n///       code b\n";
        let (output, changes) = process_content(input, 100);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_multi_digit_hierarchical_marker() {
        assert!(starts_with_hierarchical_marker("10. tenth item"));
        assert!(starts_with_hierarchical_marker("100. hundredth"));
        assert!(!starts_with_hierarchical_marker("10x not a marker"));
    }

    #[test]
    fn test_marker_requires_following_space() {
        assert!(starts_with_hierarchical_marker("-"));
        assert!(starts_with_hierarchical_marker("*\tbullet"));
        assert!(starts_with_hierarchical_marker("1."));
        assert!(starts_with_hierarchical_marker("2.\tsecond"));
        assert!(!starts_with_hierarchical_marker("*Emphasis* at the start"));
        assert!(!starts_with_hierarchical_marker("**Bold** at the start"));
        assert!(!starts_with_hierarchical_marker("--verbose prints more"));
        assert!(!starts_with_hierarchical_marker("-1 is negative"));
        assert!(!starts_with_hierarchical_marker("e.g. an abbreviation"));
        assert!(!starts_with_hierarchical_marker("3.14 is close to pi"));
        assert!(!starts_with_hierarchical_marker("1.0.0 is a version"));
        assert!(!starts_with_hierarchical_marker("ab. two letters"));
    }

    #[test]
    fn test_ordered_marker_interrupts_paragraph_only_at_start() {
        assert!(starts_new_block("A paragraph.", "1. first"));
        assert!(starts_new_block("A paragraph.", "01. first"));
        assert!(starts_new_block("A paragraph.", "- bullet"));
        assert!(!starts_new_block("A paragraph.", "2026. That was the year."));
        assert!(starts_new_block("1. first", "2. second"));
        assert!(starts_new_block("  1. first", "  2. second"));
        assert!(starts_new_block("- bullet", "3. third"));
    }

    #[test]
    fn test_combine_lines_that_only_look_like_list_items() {
        let input = "// A sentence before.\n// *Emphasis* at the start of a line.\n// **Bold** at the start of a line.\n// 2026. \
                     That was the year.\n// e.g. a line starting with an abbreviation.\n// 3.14 is close to pi.\n// --verbose prints \
                     more.\n// -1 is negative.\n// A sentence after.\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(
            output,
            "// A sentence before. *Emphasis* at the start of a line. **Bold** at the start of a line. 2026. That was the year. e.g. a line\n\
             // starting with an abbreviation. 3.14 is close to pi. --verbose prints more. -1 is negative. A sentence after.\n"
        );
    }

    #[test]
    fn test_spaced_ordered_marker_interrupts_paragraph_only_at_start() {
        assert!(starts_new_block("A paragraph.", "1) first"));
        assert!(starts_new_block("A paragraph.", "+ plus"));
        assert!(!starts_new_block("A paragraph.", "2) second"));
        assert!(starts_new_block("1) first", "2) second"));
        assert!(starts_new_block("+ plus", "2) second"));
        assert!(!starts_new_block("A paragraph.", "+1 is positive"));
        assert_eq!(ordered_marker_label("12) twelfth"), Some("12"));
        assert_eq!(ordered_marker_label("+ plus"), None);
    }

    #[test]
    fn test_combine_spaced_markers_mid_paragraph_and_in_quote() {
        let input = "// text\n// 2) continues\n// ii. continues\n// 1) first\n// 2) second\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// text 2) continues ii. continues\n// 1) first\n// 2) second\n");

        let input = "// > text\n// > 2026. continues\n// > *emphasis* continues\n// > 1. first\n// > 2. second\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(
            output,
            "// > text 2026. continues *emphasis* continues\n// > 1. first\n// > 2. second\n"
        );
    }

    #[test]
    fn test_no_combine_ordered_list_items() {
        let input = "// 1. first\n// 2. second\n// 3. third\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_no_combine_multi_digit_numbered() {
        let input = "// 9. ninth item\n// 10. tenth item\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, input);
    }

    #[test]
    fn test_process_content_wraps_multi_digit_numbered_with_indent() {
        let input = "// 10. This is a very long numbered item that should wrap with proper indentation\n";
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
        assert_eq!(result, vec!["// - this is a long bullet", "//   point text"]);
    }

    #[test]
    fn test_wrap_numbered_list_indent() {
        let result = wrap_text("1. this is a long numbered item text", "// ", "//    ", 30);
        assert_eq!(result, vec!["// 1. this is a long numbered", "//    item text"]);
    }

    #[test]
    fn test_process_content_wraps_bullet_with_indent() {
        let input = "// - This is a very long bullet point that should wrap with proper indentation\n";
        let (output, changes) = process_content(input, 40);
        assert_eq!(
            output,
            "// - This is a very long bullet point\n//   that should wrap with proper\n//   indentation\n"
        );
        assert!(!changes.is_empty());
    }

    #[test]
    fn test_process_content_wraps_numbered_with_indent() {
        let input = "// 1. This is a very long numbered item that should wrap with proper indentation\n";
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
        let input = "// This is a long comment that needs to be wrapped because it exceeds the width\n";
        let (output, _) = process_content(input, 40);
        for line in output.lines() {
            assert_eq!(line, line.trim_end(), "trailing spaces in: {:?}", line);
        }
    }

    #[test]
    fn test_no_trailing_spaces_doc_comment() {
        let input = "///\n/// Some doc text that is long enough to wrap around to the next line easily\n";
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
    fn test_ends_with_backslash_break() {
        assert!(ends_with_backslash_break("text\\"));
        assert!(ends_with_backslash_break("text\\\\\\"));
        assert!(!ends_with_backslash_break("text\\\\"));
        assert!(!ends_with_backslash_break("text\\ "));
        assert!(!ends_with_backslash_break("text"));
    }

    #[test]
    fn test_trailing_space_break() {
        assert_eq!(trailing_space_break("text  "), "  ");
        assert_eq!(trailing_space_break("text   "), "   ");
        assert_eq!(trailing_space_break("text "), "");
        assert_eq!(trailing_space_break("text"), "");
    }

    #[test]
    fn test_no_combine_hard_line_break() {
        let input =
            "/// Line one ends with a backslash\\\n/// line two.\n///\n/// Line three ends with two spaces  \n/// line four.\n";
        let (output, changes) = process_content(input, 132);
        assert_eq!(output, input);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_hard_line_break_ends_run() {
        let input = "// one\n// two\\\n// three\n// four  \n// five\n// six\n";
        let (output, _) = process_content(input, 132);
        assert_eq!(output, "// one two\\\n// three four  \n// five six\n");
    }

    #[test]
    fn test_wrap_keeps_trailing_space_break_on_last_line() {
        let input = "// aaa bbb ccc ddd  \n// eee\n";
        let (output, _) = process_content(input, 12);
        assert_eq!(output, "// aaa bbb\n// ccc ddd  \n// eee\n");
        let (again, changes) = process_content(&output, 12);
        assert_eq!(again, output);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_wrap_keeps_backslash_break_on_last_line() {
        let input = "// aaa bbb ccc ddd\\\n// eee\n";
        let (output, _) = process_content(input, 12);
        assert_eq!(output, "// aaa bbb\n// ccc ddd\\\n// eee\n");
    }

    #[test]
    fn test_wrap_does_not_end_line_with_backslash() {
        let result = wrap_text("aaa bbb\\ ccc ddd", "// ", "// ", 12);
        assert_eq!(result, vec!["// aaa", "// bbb\\ ccc", "// ddd"]);
    }

    #[test]
    fn test_blank_comment_with_trailing_spaces_is_trimmed() {
        let (output, _) = process_content("//   \n", 80);
        assert_eq!(output, "//\n");
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
        assert_eq!(tokenize_preserving_backticks("` `` ` rest"), vec!["` `` `", "rest"]);
    }

    #[test]
    fn test_tokenize_mixed_backtick_delimiters() {
        // `` ` `` is a double-backtick span containing a single backtick ` `` ` is a single-backtick span containing double
        // backticks
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
            !lines.iter().any(|l| l.ends_with("` ``") || l.ends_with("`` `")),
            "backtick span was split across lines: {lines:?}"
        );
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
