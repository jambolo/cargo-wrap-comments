# cargo-wrap-comments

[![Rust (master)](https://github.com/jambolo/cargo-wrap-comments/actions/workflows/rust.yml/badge.svg?branch=master)](https://github.com/jambolo/cargo-wrap-comments/actions/workflows/rust.yml?query=branch%3Amaster)
[![Rust (develop)](https://github.com/jambolo/cargo-wrap-comments/actions/workflows/rust.yml/badge.svg?branch=develop)](https://github.com/jambolo/cargo-wrap-comments/actions/workflows/rust.yml?query=branch%3Adevelop)

A Rust command-line tool that formats source code comments to wrap long lines at a specified width. This does what rustfmt should do.

## Features

- Wraps long comments to a configurable width (default: 100 characters)
- Reads `max_width` from the rustfmt config file, located exactly as rustfmt locates it, if `--max-width` is not specified
- Combines consecutive comment lines before wrapping
- Supports Rust comment styles: `//`, `///`, `//!`
- Preserves indentation, comment markers, and blank comment lines
- Respects hierarchical markers (`#`, `*`, `-`, `+`, `1.`, `1)`, `A.`, `a)`, `ii.`), back-tick quoted text, and code blocks
- Glob pattern support for processing multiple files
- Stdin/stdout mode when no files are specified
- Check mode for previewing changes without modifying files

## Limitations

- The tool is not intended to format markdown text in comments, although it does its best to respect markdown formatting.
- Only markers or certain formatting are considered when joining adjacent lines. The tool cannot discern context or distinguish between code and prose. Separating two lines with a blank line or terminating the first with a backslash, '\\', will always prevent them from being joined.
- The tool does not parse rust syntax, so text inside string literals that looks like a comment may be interpreted as a comment.
- Trailing comments are not wrapped.
- Lines with long text that cannot be split may not wrap as you might expect.

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
| `--config-path PATH`   | rustfmt config file or directory |         |
| `--check`              | Preview mode, don't modify files |         |
| `-v`, `--verbose`      | Print verbose output             |         |
| `-q`, `--quiet`        | Print less output                |         |
| `-V`, `--version`      | Show version information         |         |
| `-h`, `--help`         | Show help                        |         |

## Comment Combining Rules

Consecutive comment lines are combined before wrapping when ALL of these are true:

1. Same comment marker (`//`, `///`, or `//!`)
2. Same indentation level
3. Same block quote depth (the number of leading `>`). The rules below apply to the text after the `>` prefix, and the prefix is repeated on every wrapped line.
4. Neither line is blank
5. The next line does not start with a hierarchical marker: `#`, `*`, `-`, `` ``` ``, `~~~`, `1.`, `A.`, `a.`, or one of these followed by a space: `+`, `1)`, `a)`, a Roman numeral followed by `.` or `)` (e.g. `ii.`, `XII)`)
6. The current line does not start with a code fence (`` ``` `` or `~~~`) or contain `` ``` ``
7. The current line does not start with `#` (markdown heading lines are not combined with the text that follows)
8. Neither line is a divider (four or more of the same punctuation character, e.g. `----`, `====`, `****`)
9. Neither line is a markdown thematic break (up to three spaces of indentation, then three or more matching `-`, `_`, or `*` characters, optionally separated by spaces or tabs, e.g. `---`, `***`, `_ _ _`)
10. The current line does not end with a markdown hard line break (a backslash, or two or more spaces). Wrapping keeps the break at the end of the last output line, and never moves a backslash to the end of a line.
11. Neither line is a markdown table row (starts with `|`). Table rows are never wrapped, even when wider than the line width.
12. Neither line starts with an SPDX tag (e.g. `SPDX-License-Identifier:`). SPDX lines are also never wrapped.
13. The next line does not start with an annotation tag (`TODO`, `FIXME`, `NOTE`, `HACK`, `XXX` or `SAFETY`, optionally followed by `(...)`, then `:`). Lines after a tagged line may still be combined into it.

Lines inside code blocks are never combined or wrapped. This includes fenced code blocks (`` ``` `` or `~~~`) and markdown indented code blocks: text indented four or more spaces (counted from the start of a list item's content inside a list item) after a blank comment line or at the start of a comment block, continuing until a non-blank line with less indentation. A fenced code block is closed only by a fence of the same character (`` ` `` or `~`) that is at least as long as the opening fence.

## Width Resolution

The line width is resolved in this order:

1. `--max-width N` on the command line
2. `max_width` from the rustfmt config file
3. Default: 100

The config file is located the same way rustfmt locates it:

- With `--config-path PATH`: if `PATH` is a file, that file is used. If it is a directory, the `.rustfmt.toml` or `rustfmt.toml` directly inside it is used; its parent directories are not searched. Otherwise it is an error.
- Otherwise, for each file, the first `.rustfmt.toml` or `rustfmt.toml` found in this order (`.rustfmt.toml` first within each directory):
  1. The file's directory, then each parent directory up to the filesystem root
  2. The home directory
  3. `rustfmt` in the user config directory (`$XDG_CONFIG_HOME` or `~/.config` on Linux, `~/Library/Application Support` on macOS, `%APPDATA%` on Windows)
- For stdin, the search starts in the current directory.

The search stops at the first config file found. If it has no `max_width`, the default is used. A config file that is not valid TOML, or whose `max_width` is not a non-negative integer, is an error, even when `--max-width` is given. All config files are checked before any file is modified.

## Exit Codes

- `0` — success
- `1` — one or more errors occurred (missing files, processing failures)
