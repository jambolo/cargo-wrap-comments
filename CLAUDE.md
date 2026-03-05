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

### Dependencies

- `clap` (derive) for CLI parsing
- `glob` for file pattern expansion
- `toml` for rustfmt config parsing
