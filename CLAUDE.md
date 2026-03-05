# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

- `cargo build` — build debug binary
- `cargo build --release` — build release binary
- `cargo test` — run all tests
- `cargo test test_name` — run a single test by name

## Architecture

Single-file Rust CLI tool in `src/main.rs`. No library crate.

### Processing Pipeline

1. **Parse**: `parse_comment_line()` extracts indent, marker (`//`, `///`, `//!`), and text from each line
2. **Combine**: consecutive comment lines are joined if they pass `can_combine()` checks (same marker, same indent, neither blank, no hierarchical markers or code fences on boundaries)
3. **Code block tracking**: `process_content()` tracks `` ``` `` toggles within comments; lines inside code blocks pass through unchanged
4. **Wrap**: `wrap_text()` breaks combined text at word boundaries to fit within `max_width`
5. **Diff**: changes are tracked as `(line_number, old_lines, new_lines)` tuples for `--check` output

### Width Resolution

`resolve_width()` checks in order: `--width` CLI arg, `max_width` from `.rustfmt.toml`/`rustfmt.toml` (searched from cwd up to `$HOME`), then default 100.

### Key Types

- `CommentLine` — parsed comment with `indent`, `marker`, `text` fields
- `Cli` — clap derive struct with `files`, `width`, `check`

### Dependencies

- `clap` (derive) for CLI parsing
- `glob` for file pattern expansion
- `toml` for rustfmt config parsing
