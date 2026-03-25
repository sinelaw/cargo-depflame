# Implementation Plan: Heuristic-Based Upstream Dependency Analyzer

## Overview

A **stable-Rust-compatible** Cargo subcommand (`cargo-depflame`) that identifies
high-ROI opportunities to reduce transitive dependency bloat by submitting upstream PRs.
Instead of compiler-level MIR analysis, it uses dependency graph weighting and
lexical/AST heuristics to find intermediate crates that pull in massive dependency
trees but barely use them.

### Core Metric — Heuristic URRS (hURRS)

```
hURRS = W_transitive(C_heavy) / C_ref(C_heavy inside C_intermediate)
```

A high hURRS means an intermediate crate drags in a huge transitive tree for
very few code references — a prime target for an upstream feature-flag PR.

---

## Phase 1: Project Scaffolding & CLI

### Goal
Set up the Cargo subcommand binary, CLI argument parsing, and basic project structure.

### Tasks

1. **Initialize Cargo project**
   - `cargo init --name cargo-depflame`
   - Binary crate; Cargo will invoke it as `cargo depflame` via the
     `cargo-<subcommand>` naming convention.

2. **Add core dependencies to `Cargo.toml`**
   ```toml
   [dependencies]
   clap = { version = "4", features = ["derive"] }
   cargo_metadata = "0.18"
   serde = { version = "1", features = ["derive"] }
   serde_json = "1"
   ```

3. **Define CLI with `clap`**
   - `src/main.rs`: top-level entry point.
   - `src/cli.rs`: define the CLI using `clap::Parser`.
   - Subcommands:
     - `analyze` — run the full pipeline (graph weighting → source scan → report).
     - `report` — re-render a previously saved JSON analysis.
   - Common flags:
     - `--manifest-path <path>` — path to root `Cargo.toml` (default: current dir).
     - `--threshold <f64>` — minimum hURRS score to report (default: 3.0).
     - `--top <n>` — only show top N results (default: 10).
     - `--format <text|json>` — output format (default: text).
     - `--output <path>` — write report to file instead of stdout.

4. **Create module skeleton**
   ```
   src/
   ├── main.rs          # Entry point, delegates to cli
   ├── cli.rs           # Clap definitions
   ├── graph.rs         # Dependency graph construction & weighting (Phase 2)
   ├── registry.rs      # Cargo registry source location (Phase 3)
   ├── scanner.rs       # Heuristic source scanning (Phase 4)
   ├── metrics.rs       # hURRS and related metric calculation (Phase 5)
   ├── report.rs        # Report rendering — text & JSON (Phase 6)
   └── error.rs         # Unified error types (thiserror or anyhow)
   ```

5. **Wire up a smoke-test end-to-end path**
   - `analyze` subcommand calls stubs for each phase and prints a placeholder message.
   - Confirm `cargo install --path .` works and `cargo depflame analyze`
     runs without error.

### Acceptance Criteria
- `cargo depflame --help` prints usage.
- `cargo depflame analyze` exits cleanly (no-op) on any Cargo workspace.

---

## Phase 2: Dependency Graph Construction & Weighting

### Goal
Build an in-memory dependency graph from `cargo_metadata` and compute transitive
weight (`W_transitive`) for every node.

### Tasks

1. **Run `cargo_metadata`**
   - In `graph.rs`, invoke `cargo_metadata::MetadataCommand::new()` with the
     user-provided manifest path.
   - Parse the returned `Metadata`, focusing on `resolve` (the dependency
     resolution graph) and `packages` (package metadata).

2. **Build an adjacency list representation**
   - `struct DepGraph` holding:
     - `HashMap<PackageId, DepNode>` — node data per package.
     - `HashMap<PackageId, Vec<PackageId>>` — forward edges (dependency → dependents).
     - `HashMap<PackageId, Vec<PackageId>>` — reverse edges (dependent → dependencies).
   - `struct DepNode`:
     - `name: String`
     - `version: semver::Version`
     - `source: Option<String>` (registry URL or path)
     - `is_workspace_member: bool`
     - `features_enabled: Vec<String>`
     - `transitive_weight: usize` (computed in step 3)

3. **Compute `W_transitive` for every node**
   - For each node, perform a BFS/DFS over its forward edges to count the
     number of **unique** transitive dependencies (including itself).
   - Cache results to avoid recomputation (memoized DFS with `HashMap<PackageId, usize>`).
   - Store the result in `DepNode::transitive_weight`.

4. **Identify "Fat Nodes"**
   - Flag every non-workspace package where `W_transitive > threshold`
     (configurable, default 10).
   - Return `Vec<FatNode>` containing the package id, name, version, and weight.

5. **Identify "Intermediate Edges"**
   - For each fat node F, walk the reverse edges to find intermediate crates I
     such that:
     - I is a direct or transitive dependency of a workspace member.
     - I depends on F.
     - I is **not** a workspace member (we want to target upstream crates).
   - Collect pairs `(I, F)` — each is a candidate for heuristic scanning.

6. **Unit tests**
   - Test graph construction with a mock `Metadata` fixture.
   - Test transitive weight calculation on a known small graph.
   - Test fat-node identification with various thresholds.

### Acceptance Criteria
- Given a real workspace, `analyze` prints a list of fat nodes with their
  transitive weights.

---

## Phase 3: Source Code Retrieval from Cargo Registry

### Goal
Locate the downloaded source code of intermediate crates in the local Cargo
registry cache so we can scan how they use a fat dependency.

### Tasks

1. **Determine Cargo home directory**
   - In `registry.rs`, read `$CARGO_HOME` (default `~/.cargo`).

2. **Locate registry source directories**
   - The standard path is `$CARGO_HOME/registry/src/<registry-hash>/`.
   - List directories matching this pattern; typically there is one per registry
     (crates.io).

3. **Resolve a `(name, version)` pair to a source path**
   - Given an intermediate crate's name and version (from `cargo_metadata`),
     look for a directory named `{name}-{version}` under the registry src dir.
   - Return `Option<PathBuf>`.
   - Handle edge cases:
     - Crate not in cache (user hasn't built yet) → warn and skip.
     - Path dependencies or git dependencies → attempt to resolve from the
       `source` field in metadata; skip if unavailable.

4. **Collect all `.rs` source files for a crate**
   - Given a crate's source root, recursively glob for `**/*.rs`.
   - Return `Vec<PathBuf>`.

5. **Integration test**
   - On a workspace that has been built at least once, verify we can locate
     source for a known transitive dependency.

### Acceptance Criteria
- For each `(intermediate, fat_node)` pair from Phase 2, we can either locate
  the intermediate crate's source or emit a clear warning.

---

## Phase 4: Heuristic Usage Scanning

### Goal
Count how many times an intermediate crate references the fat dependency in its
source code, producing `C_ref`.

### Design Decision: Lexical vs AST Scanning

Implement **both** approaches behind a strategy enum, defaulting to lexical:

- **Lexical (default)**: Fast regex scan. Good enough for most cases.
- **AST (opt-in via `--ast` flag)**: More accurate, uses `syn` to parse source
  and walk paths. Slower but avoids false positives from comments/strings.

### Tasks

1. **Define the scanner trait**
   ```rust
   // In scanner.rs
   pub struct ScanResult {
       pub crate_name: String,
       pub ref_count: usize,          // C_ref
       pub file_matches: Vec<FileMatch>,
   }

   pub struct FileMatch {
       pub path: PathBuf,
       pub line_number: usize,
       pub line_content: String,
   }
   ```

2. **Implement lexical scanner**
   - Add `regex` to dependencies.
   - For a given `(intermediate_src_files, fat_crate_name)`:
     - Compile pattern: `\b{fat_crate_name}::` (escaped for regex).
     - Also match `use {fat_crate_name}` and `extern crate {fat_crate_name}`.
     - Iterate over each `.rs` file, scan line-by-line.
     - Collect matches into `ScanResult`.
   - Handle crate name normalization: Cargo normalizes hyphens to underscores
     in Rust code, so search for the underscore-normalized form.

3. **Implement AST scanner (opt-in)**
   - Add `syn = { version = "2", features = ["full", "visit"] }` to dependencies.
   - Implement `syn::visit::Visit` on a custom visitor struct.
   - In `visit_use_tree`, `visit_path`, and `visit_path_segment`:
     - Check if the first segment matches the fat crate name.
     - Increment counter and record location (using `span` for approximate line).
   - Parse each `.rs` file with `syn::parse_file`.
   - Gracefully handle parse failures (warn and fall back to lexical for that file).

4. **Skip non-code content in lexical mode**
   - Simple heuristic: skip lines starting with `//` or within `/* ... */` blocks.
   - Not perfect, but reduces false positives cheaply.

5. **Unit tests**
   - Test with synthetic Rust source containing various import styles:
     `use foo::Bar`, `foo::baz()`, `extern crate foo`, comments containing `foo::`.
   - Verify AST scanner handles `use foo::{A, B}` as a single reference vs
     multiple (design choice: count as 1 `use` statement = 1 ref).

### Acceptance Criteria
- For each `(intermediate, fat_node)` pair, produce a `ScanResult` with
  accurate-enough `C_ref` and file-level match details.

---

## Phase 5: Metric Calculation & Ranking

### Goal
Compute hURRS scores and rank the results to surface the highest-ROI upstream
contribution opportunities.

### Tasks

1. **Define metric structures in `metrics.rs`**
   ```rust
   pub struct UpstreamTarget {
       pub intermediate: PackageInfo,    // the crate to submit a PR to
       pub fat_dependency: PackageInfo,  // the heavy dep to remove/feature-gate
       pub w_transitive: usize,          // transitive weight of fat dep
       pub c_ref: usize,                 // reference count in intermediate
       pub hurrs: f64,                   // W_transitive / C_ref
       pub scan_result: ScanResult,      // detailed file matches
       pub suggestion: RemovalStrategy,  // recommended action
   }

   pub enum RemovalStrategy {
       FeatureGate,          // put behind a feature flag
       ReplaceWithStd,       // can be replaced with std functionality
       ReplaceWithLighter,   // can use a lighter alternative crate
       Remove,               // appears entirely unused (C_ref = 0)
   }
   ```

2. **Compute hURRS**
   - For each `(intermediate, fat_node, scan_result)` triple:
     - `hurrs = w_transitive as f64 / c_ref.max(1) as f64`
     - If `c_ref == 0`, set `hurrs = f64::INFINITY` and mark as `Remove`.
     - Otherwise, default suggestion to `FeatureGate`.

3. **Filter and rank**
   - Discard results where `hurrs < threshold` (CLI flag `--threshold`).
   - Sort remaining by `hurrs` descending (highest ROI first).
   - Truncate to `--top N`.

4. **Detect std-replaceable patterns (stretch goal)**
   - Maintain a small built-in mapping of common crates to std equivalents:
     - `lazy_static` / `once_cell` → `std::sync::OnceLock` (Rust 1.80+)
     - `regex` for trivial patterns → `str::contains`, `str::starts_with`
   - If the fat crate name matches a known entry AND `c_ref` is low,
     upgrade the suggestion to `ReplaceWithStd` and include the recommended
     replacement in the report.

5. **Tests**
   - Verify ranking order with known inputs.
   - Verify threshold filtering.
   - Verify `c_ref = 0` produces `Remove` suggestion.

### Acceptance Criteria
- Produce a ranked `Vec<UpstreamTarget>` ready for rendering.

---

## Phase 6: Report Generation

### Goal
Render results as human-readable terminal output and machine-readable JSON.

### Tasks

1. **Add display dependencies**
   ```toml
   comfy-table = "7"
   colored = "2"
   ```

2. **JSON output (`--format json`)**
   - Serialize `Vec<UpstreamTarget>` with serde_json.
   - Include metadata: tool version, timestamp, workspace root, threshold used.
   - Write to `--output` path or stdout.

3. **Text output (`--format text`, default)**
   - **Summary header**: workspace name, total dependencies, total fat nodes found.
   - **Top targets list**: For each `UpstreamTarget`, render a block like:

     ```
     --- #1 (hURRS: 20.0) -------------------------------------------
     Upstream Crate:      cli-table v0.4.7
     Offending Dependency: csv (brings in 15 transitive crates)
     References Found:     3 across 1 file

     Files:
       ~/.cargo/registry/src/.../cli-table-0.4.7/src/export.rs
         L42: use csv::Writer;
         L87: let writer = csv::Writer::from_writer(buf);
         L91: csv::Writer::flush(&mut writer);

     Suggested Action: FEATURE GATE
       Put `csv` behind a Cargo feature flag in cli-table's Cargo.toml:
         [features]
         csv-export = ["dep:csv"]
         [dependencies]
         csv = { version = "1", optional = true }

       This would drop 15 transitive dependencies for users who don't
       need CSV export.
     ```

   - **Summary table** at the end using `comfy-table`:
     | # | Upstream Crate | Fat Dep | W_trans | C_ref | hURRS | Action |

4. **Report subcommand**
   - `cargo depflame report --input analysis.json` reads a saved JSON
     and re-renders it (useful for CI pipelines that separate analysis from display).

5. **Tests**
   - Snapshot tests for text output format.
   - Round-trip test: serialize to JSON, deserialize, re-render text.

### Acceptance Criteria
- Both text and JSON output are complete, correct, and readable.
- JSON output can be consumed by the `report` subcommand.

---

## Phase 7: Error Handling, Polish & Packaging

### Goal
Harden the tool for real-world use, add documentation, and prepare for distribution.

### Tasks

1. **Robust error handling**
   - Use `anyhow` for application errors with context.
   - Graceful degradation:
     - Crate source not in cache → skip with warning, don't abort.
     - `syn` parse failure → fall back to lexical scan for that file.
     - Workspace with no external dependencies → clean exit with message.

2. **Logging & verbosity**
   - Add `env_logger` or `tracing-subscriber` for `--verbose` / `-v` flag.
   - Log: which crates are being scanned, cache misses, parse failures.

3. **Performance considerations**
   - Use `rayon` for parallel file scanning across multiple intermediate crates.
   - Avoid reading files larger than 1MB (likely generated/vendored code).

4. **Documentation**
   - `README.md` with usage examples and sample output.
   - `--help` text for all subcommands and flags.
   - Inline doc comments on all public types and functions.

5. **CI setup**
   - GitHub Actions workflow:
     - `cargo test` on stable
     - `cargo clippy`
     - `cargo fmt --check`

6. **Integration tests**
   - Create a `tests/fixtures/` workspace with a known dependency graph.
   - Pin specific crate versions so results are deterministic.
   - Assert expected fat nodes, hURRS scores, and report content.

### Acceptance Criteria
- `cargo clippy` and `cargo test` pass cleanly.
- Tool runs without panic on real-world workspaces (test on 2-3 popular
  open-source Rust projects).

---

## Dependency Summary

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
cargo_metadata = "0.18"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
regex = "1"
syn = { version = "2", features = ["full", "visit"], optional = true }
comfy-table = "7"
colored = "2"
anyhow = "1"
semver = "1"
rayon = "1"

[features]
default = []
ast-scanner = ["dep:syn"]
```

## Non-Goals (Out of Scope)

- **Compiler integration / MIR analysis** — that's the full tool (Sections I-VI of the design doc).
- **Automatic refactoring** — this tool only produces reports and suggestions.
- **Micro-inlining / code generation** — no source code modification.
- **Patching upstream Cargo.toml** — the user submits the PR manually.
- **Build-time measurement** — we measure dependency count, not compile time.

## Open Questions

1. **Should we also scan workspace members' usage of fat deps?**
   Potentially useful to also tell the user "you only use `regex` in 2 places
   in your own code — consider replacing." This is a natural extension of Phase 4
   but changes the tool's scope from "upstream triage" to "self triage."

2. **How to handle feature-gated transitive deps?**
   A fat dep might already be optional in the intermediate crate. We should
   check the `features` field in `cargo_metadata` and skip already-optional deps
   (or at least note it in the report).

3. **Git dependency support?**
   Git deps won't be in `~/.cargo/registry/src/`. We could check
   `~/.cargo/git/checkouts/` but the directory naming is less predictable.
   Initial version: warn and skip.
