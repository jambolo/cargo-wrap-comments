# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Wrapping Rules (`wrap_text()`)

- Only triggers when the combined line exceeds `max_width`
- Splits at whitespace boundaries
- Backtick-delimited spans (`` ` `` or `` `` ``) are tokenized as single units and never split
- Words exceeding available width are emitted unsplit on their own line
- Continuation lines after hierarchical markers get extra indent matching the marker width:
  - `-` (space) or `*` (space) → 2 chars
  - `+` (space) → 2 chars
  - `N.` or `N)` (space, 1 to 9 digits like `1.` or `10)`) → digit count + 2 chars
  - Letters and Roman numerals (`A.`, `a)`, `ii.`) are not markers, as in CommonMark
  - `#` (space, headings) → hash count + 1 chars
