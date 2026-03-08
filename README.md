# cargo-wrap-comments

[![Rust (master)](https://github.com/jambolo/cargo-wrap-comments/actions/workflows/rust.yml/badge.svg?branch=master)](https://github.com/jambolo/cargo-wrap-comments/actions/workflows/rust.yml?query=branch%3Amaster)
[![Rust (develop)](https://github.com/jambolo/cargo-wrap-comments/actions/workflows/rust.yml/badge.svg?branch=develop)](https://github.com/jambolo/cargo-wrap-comments/actions/workflows/rust.yml?query=branch%3Adevelop)

A Rust command-line tool that formats source code comments to wrap long lines at a specified width. This does what rustfmt should do.

## Features

- Wraps long comments to a configurable width (default: 100 characters)
- Reads `max_width` from `.rustfmt.toml` or `rustfmt.toml` if `--max-width` is not specified
- Combines consecutive comment lines before wrapping
- Supports Rust comment styles: `//`, `///`, `//!`
- Preserves indentation, comment markers, and blank comment lines
- Respects hierarchical markers (`#`, `*`, `-`, `1.`, `A.`, `a.`), back-tick quoted text, and code blocks
- Glob pattern support for processing multiple files
- Stdin/stdout mode when no files are specified
- Check mode for previewing changes without modifying files

## Installation

```sh
cargo install cargo-wrap-comments
```

### From source

```sh
cargo build --release
```

The binary will be at `target/release/cargo-wrap-comments`.

## Usage

The tool can be run as a standalone command or as a Cargo subcommand:

```sh
# As a cargo subcommand
cargo wrap-comments src/main.rs

# As a standalone command
cargo-wrap-comments src/main.rs

# Format comments in multiple files with glob patterns
cargo wrap-comments "src/**/*.rs"

# Set custom width
cargo wrap-comments --max-width 80 src/main.rs

# Preview changes without modifying files
cargo wrap-comments --check src/main.rs

# Read from stdin, write to stdout
cat src/main.rs | cargo wrap-comments
cargo wrap-comments < src/main.rs
```

## Options

| Option                 | Description                      | Default |
|------------------------|----------------------------------|---------|
| `-w`, `--max-width N`  | Maximum line width               | 100     |
| `--check`              | Preview mode, don't modify files |         |
| `-v`, `--verbose`      | Print verbose output             |         |
| `-q`, `--quiet`        | Print less output                |         |
| `-V`, `--version`      | Show version information         |         |
| `-h`, `--help`         | Show help                        |         |

## Comment Combining Rules

Consecutive comment lines are combined before wrapping when ALL of these are true:

1. Same comment marker (`//`, `///`, or `//!`)
2. Same indentation level
3. Neither line is blank
4. The next line does not start with: `#`, `*`, `-`, `1.`, `A.`, `a.`
5. The next line does not contain a code block marker (`` ``` ``)

## Width Resolution

The line width is resolved in this order:

1. `--max-width N` on the command line
2. `max_width` from `.rustfmt.toml` or `rustfmt.toml`, searching from the current directory up to `$HOME`
3. Default: 100

The search stops at the first config file found. If it exists but has no `max_width`, the default is used.

## Exit Codes

- `0` — success
- `1` — one or more errors occurred (missing files, processing failures)
