# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

- `cargo build` — build debug binary
- `cargo build --release` — build release binary
- `cargo test` — run all tests
- `cargo test test_name` — run a single test by name

## Architecture

Single-file Rust CLI tool in `src/main.rs`. No library crate. Binary name: `cargo-wrap-comments`.

### Cargo Subcommand Pattern

Uses a two-level clap enum: `CargoCli::WrapComments(Cli)`. This allows `cargo wrap-comments` to work as a cargo subcommand. The `CargoCli` enum has `#[command(name = "cargo")]` and the variant has `#[command(name = "wrap-comments")]`.

### Processing Pipeline

1. **Parse**: `parse_comment_line()` extracts indent, marker (`//`, `///`, `//!`), and text from each line
2. **Combine**: consecutive comment lines are joined if they pass `can_combine()` checks (same marker, same indent, neither blank, no hierarchical markers or code fences on boundaries)
3. **Code block tracking**: `process_content()` tracks `` ``` `` toggles within comments; lines inside code blocks pass through unchanged
4. **Wrap**: `wrap_text()` breaks combined text at word boundaries to fit within `max_width`; `tokenize_preserving_backticks()` keeps backtick-delimited spans unsplit
5. **Diff**: changes are tracked as `(line_number, old_lines, new_lines)` tuples for `--check` output

### Width Resolution

`resolve_width()` checks in order: `--max-width` / `-w` CLI arg, `max_width` from `.rustfmt.toml`/`rustfmt.toml` (searched from cwd up to `$HOME`), then default 100.

### Key Types

- `CommentLine` — parsed comment with `indent`, `marker`, `text` fields
- `Cli` — clap derive struct with `files`, `max_width`, `check`, `verbose`, `quiet`

### Joining Rules (`can_combine()`)

Two adjacent comment lines are joined into one when ALL of these hold:

1. Same `marker` (`//`, `///`, or `//!`)
2. Same `indent` (leading whitespace)
3. Neither line has empty text (blank comments are paragraph separators)
4. Next line does not start with a hierarchical marker
5. Current line does not contain a code fence (`` ``` ``)

Lines inside a code block (between `` ``` `` toggles) are never joined or wrapped.

### Hierarchical Markers (`starts_with_hierarchical_marker()`)

These patterns at the start of comment text prevent joining with the previous line:

- `#` (markdown headings)
- `*` or `-` (bullet lists)
- `` ``` `` (code fences)
- One or more digits followed by `.` (e.g. `1.`, `10.`, `100.`)
- Single letter followed by `.` (e.g. `A.`, `a.`)

### Wrapping Rules (`wrap_text()`)

- Only triggers when the combined line exceeds `max_width`
- Splits at whitespace boundaries
- Backtick-delimited spans (`` ` `` or `` `` ``) are tokenized as single units and never split
- Words exceeding available width are emitted unsplit on their own line
- Continuation lines after hierarchical markers get extra indent matching the marker width:
  - `- ` or `* ` → 2 chars
  - `X. ` (single letter) → 3 chars
  - `N. ` (multi-digit number like `10.`) → digit count + 2 chars
  - `# ` headings → hash count + 1 chars

### Dependencies

- `clap` (derive) for CLI parsing
- `glob` for file pattern expansion
- `toml` for rustfmt config parsing
