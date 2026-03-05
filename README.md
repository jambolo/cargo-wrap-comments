# comment_formatter

[![Rust (master)](https://github.com/jambolo/comment_formatter/actions/workflows/rust.yml/badge.svg?branch=master)](https://github.com/jambolo/comment_formatter/actions/workflows/rust.yml?query=branch%3Amaster)
[![Rust (develop)](https://github.com/jambolo/comment_formatter/actions/workflows/rust.yml/badge.svg?branch=develop)](https://github.com/jambolo/comment_formatter/actions/workflows/rust.yml?query=branch%3Adevelop)

A Rust command-line tool that formats source code comments to wrap long lines at a specified width. This does what rustfmt should do.

## Features

- Wraps long comments to a configurable width (default: 100 characters)
- Reads `max_width` from `.rustfmt.toml` or `rustfmt.toml` if `--width` is not specified
- Combines consecutive comment lines before wrapping
- Supports C/C++/Rust comment styles: `//`, `///`, `//!`
- Preserves indentation, comment markers, and blank comment lines
- Respects hierarchical markers (`#`, `*`, `-`, `1.`, `A.`, `a.`) and code blocks
- Glob pattern support for processing multiple files
- Check mode for previewing changes without modifying files

## Installation

```sh
cargo build --release
```

The binary will be at `target/release/comment_formatter`.

## Usage

```sh
# Format comments in a single file
comment_formatter src/main.rs

# Format comments in multiple files with glob patterns
comment_formatter "src/**/*.rs"

# Set custom width
comment_formatter --width 80 src/main.rs

# Preview changes without modifying files
comment_formatter --check src/main.rs
```

## Options

| Option      | Description                          | Default |
|-------------|--------------------------------------|---------|
| `--width N` | Maximum line width                   | 100     |
| `--check`   | Preview mode, don't modify files     |         |

## Comment Combining Rules

Consecutive comment lines are combined before wrapping when ALL of these are true:

1. Same comment marker (`//`, `///`, or `//!`)
2. Same indentation level
3. Neither line is blank
4. The next line does not start with: `#`, `*`, `-`, `1.`, `A.`, `a.`
5. The next line does not contain a code block marker (`` ``` ``)
6. The current line does not contain a code block marker (`` ``` ``)

## Width Resolution

The line width is resolved in this order:

1. `--width N` on the command line
2. `max_width` from `.rustfmt.toml` or `rustfmt.toml`, searching from the current directory up to `$HOME`
3. Default: 100

The search stops at the first config file found. If it exists but has no `max_width`, the default is used.

## Exit Codes

- `0` — success
- `1` — one or more errors occurred (missing files, processing failures)
