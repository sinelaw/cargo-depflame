# cargo-depflame

Visualize your Cargo dependency tree as an interactive flamegraph and get actionable suggestions to reduce your dependency count.

![depflame screenshot](screenshot.png)

## Install

```sh
cargo install --path .
```

## Usage

```sh
# Open an interactive HTML report in your browser (default command)
cargo depflame

# Text summary
cargo depflame analyze

# Verbose text with source references
cargo depflame analyze --verbose

# JSON output
cargo depflame analyze --format json

# Re-render a saved JSON report
cargo depflame report --input report.json --format html
```

## What it does

**cargo-depflame** analyzes your workspace's dependency graph and produces a self-contained HTML report with three tabs:

- **Flamegraph** -- interactive icicle chart of your full dependency tree. Click to zoom, search by name, hover to highlight shared deps. Unused dependencies are highlighted in red.
- **Suggestions** -- actionable recommendations organized by type:
  - *Remove unused dependencies* -- deps in your `Cargo.toml` with no references in source
  - *Disable unnecessary features* -- optional deps pulled in by default features you don't need, with exact `Cargo.toml` diffs
  - *Make dependencies optional* -- deps that could be behind a feature flag, with `[features]` section examples
  - *Proposals for upstream* -- changes that require a PR to an external library
- **Raw JSON** -- the full analysis data for scripting or further processing

## Key features

- **Dependency flamegraph** with click-to-zoom, search, and hover highlighting
- **Unused dep detection** for your workspace crates
- **Feature-flag analysis** -- traces which features pull in optional deps, suggests `default-features = false` with the right feature subset
- **Cargo.toml diffs** -- click "show diff" on any suggestion to see exactly what to change
- **Crates.io links** on all crate names
- **Single-file HTML report** -- self-contained, shareable, no external dependencies
- **Per-edge unused coloring** -- a dep is only marked unused under parents that don't reference it

## License

MIT
