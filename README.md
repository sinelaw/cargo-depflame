# cargo-depflame

Visualize your Cargo dependency tree as an interactive flamegraph. Find crates that pull in too many transitive deps and get concrete suggestions to slim your build.

![depflame screenshot](screenshot.png)

## Install

```sh
cargo install cargo-depflame

# Or from source
cargo install --git https://github.com/sinelaw/cargo-depflame
```

Requires Rust 1.88+.

## Usage

```sh
cargo depflame              # open interactive HTML report in browser
cargo depflame analyze      # text summary to stdout
cargo depflame analyze -v   # verbose: show source references
cargo depflame analyze --format json  # machine-readable output
```

## What it does

1. Builds your full dependency graph from `cargo metadata`
2. Finds "heavy" crates with large transitive dep trees
3. Scans source code to see how heavily each heavy dep is actually used
4. Computes W_unique: how many deps *actually disappear* if an edge is cut (accounts for diamond deps)
5. Suggests concrete actions: remove unused deps, disable default features, feature-gate, propose upstream PRs

The HTML report has three tabs: an interactive flamegraph, actionable suggestions with Cargo.toml diffs, and raw JSON.

## How is this different from cargo-udeps / cargo-machete?

Both find unused deps. cargo-depflame also:

- Analyzes the full *transitive* graph and computes real savings (W_unique), not just "is it used?"
- Detects deps that are already optional upstream and shows you which feature flags to disable
- Suggests upstream PRs for feature-gating in external crates
- Produces a visual flamegraph of your dep tree
- Works on stable (no nightly required, unlike cargo-udeps)

## Caveats

Source scanning is regex-based, not semantic. It doesn't understand block comments, string literals, macros, or cfg-gated code. Treat suggestions as leads, not commands. Every suggestion in the HTML report links to the exact source lines so you can verify.

## License

MIT
