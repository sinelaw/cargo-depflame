use crate::graph::EdgeMeta;
use crate::scanner::ScanResult;
use serde::{Deserialize, Serialize};

/// The recommended action for an upstream target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RemovalStrategy {
    /// The heavy dependency appears entirely unused (C_ref = 0).
    Remove,
    /// Put the heavy dependency behind a Cargo feature flag.
    FeatureGate,
    /// The heavy dependency can be replaced with std functionality.
    ReplaceWithStd { suggestion: String },
    /// A lighter alternative crate exists.
    ReplaceWithLighter { alternative: String },
    /// The dependency is only used in test code — move to [dev-dependencies].
    MoveToDevDeps,
    /// The dependency is already optional/gated in an upstream crate.
    AlreadyGated {
        detail: String,
        /// Feature names of the intermediate crate that enable this optional dep.
        /// Empty if we couldn't determine them.
        #[serde(default)]
        enabling_features: Vec<String>,
        /// If the enabling feature is part of default, these are the default
        /// features the user should keep (i.e. defaults minus the one pulling
        /// in the heavy dep). None if not enabled via defaults.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recommended_defaults: Option<Vec<String>>,
    },
    /// The dependency is required by a sibling dep — cannot be removed.
    RequiredBySibling { sibling: String },
    /// The dependency is small or lightly used — propose inlining the used code
    /// into the intermediate crate to eliminate the dep entirely.
    InlineUpstream {
        /// LOC of the heavy dependency crate.
        heavy_loc: usize,
        /// Number of distinct API items used.
        api_items_used: usize,
    },
}

impl std::fmt::Display for RemovalStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Remove => write!(f, "REMOVE (appears unused)"),
            Self::MoveToDevDeps => write!(f, "MOVE TO [dev-dependencies] (only used in tests)"),
            Self::FeatureGate => write!(f, "FEATURE GATE"),
            Self::ReplaceWithStd { suggestion } => {
                write!(f, "REPLACE WITH STD ({suggestion})")
            }
            Self::ReplaceWithLighter { alternative } => {
                write!(f, "REPLACE WITH LIGHTER CRATE ({alternative})")
            }
            Self::AlreadyGated {
                detail,
                enabling_features,
                ..
            } => {
                if enabling_features.is_empty() {
                    write!(f, "ALREADY GATED ({detail})")
                } else {
                    write!(
                        f,
                        "ALREADY GATED ({detail}, enabled by: {})",
                        enabling_features.join(", ")
                    )
                }
            }
            Self::RequiredBySibling { sibling } => {
                write!(f, "REQUIRED BY {sibling}")
            }
            Self::InlineUpstream {
                heavy_loc,
                api_items_used,
            } => {
                write!(
                    f,
                    "INLINE ({heavy_loc} LOC crate, {api_items_used} items used)"
                )
            }
        }
    }
}

/// Confidence level for the analysis result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Confidence {
    /// Signal is noise — dep is already gated or platform-conditional.
    Noise,
    /// Low confidence — likely a false positive (e.g., -sys crate, build dep).
    Low,
    /// Medium confidence — possible false positive (renamed dep, macro usage).
    Medium,
    /// High confidence — refs found in source, or clearly unused.
    High,
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Noise => write!(f, "NOISE"),
            Self::Low => write!(f, "LOW"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::High => write!(f, "HIGH"),
        }
    }
}

/// Information about a package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
}

/// A scored upstream target for reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamTarget {
    /// The crate that directly depends on the heavy dependency (the "middle" crate
    /// between your workspace and the bloated transitive dep).
    pub intermediate: PackageInfo,
    /// The heavy/bloated transitive dependency we're proposing to remove or gate.
    pub heavy_dependency: PackageInfo,
    /// W_transitive: total number of transitive dependencies the heavy dep pulls in
    /// (i.e., the "weight" of keeping it in the tree).
    pub w_transitive: usize,
    /// How many deps would actually disappear if this edge were cut.
    pub w_unique: usize,
    /// C_ref (Code Reference count): number of times the heavy dependency's symbols
    /// appear in the intermediate crate's source code. Zero means the dep appears
    /// entirely unused by the intermediate.
    pub c_ref: usize,
    /// hURRS (heuristic Unused Ratio Rating Score): W_transitive / C_ref.
    /// Higher means more transitive deps per reference — a better removal candidate.
    /// None means infinity (C_ref = 0, dependency appears unused).
    pub hurrs: Option<f64>,
    pub confidence: Confidence,
    /// Results from scanning the intermediate crate's source for references to
    /// the heavy dependency (ref counts, file matches, distinct API items used).
    pub scan_result: ScanResult,
    pub suggestion: RemovalStrategy,
    /// Metadata about the dependency edge between intermediate and heavy dep
    /// (e.g., whether it's optional, build-only, or platform-conditional).
    pub edge_meta: EdgeMeta,
    /// Shortest dependency chain from workspace to the heavy dependency.
    #[serde(default)]
    pub dep_chain: Vec<String>,
    /// If set, a sibling dependency of the intermediate crate transitively
    /// requires the heavy dep — so removing it would break the build.
    #[serde(default)]
    pub required_by_sibling: Option<String>,
    /// True if the heavy dependency is not in the real platform-resolved tree.
    #[serde(default)]
    pub phantom: bool,
    /// True if the intermediate crate is a workspace member (user's own crate).
    #[serde(default)]
    pub intermediate_is_workspace_member: bool,
    /// True if the intermediate is a standalone workspace member not depended
    /// on by any other workspace member — already effectively opt-in.
    #[serde(default)]
    pub is_standalone_integration: bool,
    /// Lines of code in the heavy dependency crate (0 if unknown).
    #[serde(default)]
    pub heavy_dep_loc: usize,
    /// Number of direct dependencies the heavy dep itself has.
    /// A heavy dep with 0 own deps is a leaf — potentially inlinable.
    #[serde(default)]
    pub heavy_dep_own_deps: usize,
    /// True if the intermediate crate has `pub use <heavy_dep>::*` (full re-export).
    #[serde(default)]
    pub has_re_export_all: bool,
    // TODO: Future — deep usage analysis via reachable LOC
    //
    // Currently we estimate "light usage" by counting distinct API symbols
    // referenced at call sites (e.g., "uses Adapter, from_u8"). This is a
    // rough proxy — one symbol might fan out to thousands of LOC internally.
    //
    // The ideal approach: measure how much code inside the heavy dep is actually
    // *reachable* from the entry points the intermediate crate uses. This
    // requires intra-crate call graph analysis:
    //   1. Extract fn/method definitions and their line spans
    //   2. For each definition, identify callees (other fns in the same crate)
    //   3. BFS/DFS from the used entry points through the call graph
    //   4. Sum LOC of all reachable definitions
    //   5. Report "uses ~X of Y LOC" — if X is small, inlining is practical
    //
    // Implementation options (tradeoffs):
    //   - Regex-based (tried, removed): fast to implement but too slow on large
    //     crates (>200KB source), can't handle traits/generics/macros, and regex
    //     over function bodies produces many false callee matches.
    //   - rust-analyzer APIs (ra_ap_ide / ra_ap_hir): would give real semantic
    //     call graphs including trait dispatch, generics, and macro expansion.
    //     Heavy dependency (~100+ crates) but accurate. Could run on-demand per
    //     crate rather than loading the whole workspace.
    //   - cargo check + MIR: drive rustc to emit MIR, then analyze reachability
    //     at the MIR level. Most accurate (post-monomorphization) but requires
    //     compiling each crate. Could cache results.
    //   - syn-based AST parsing: lighter than rust-analyzer, can extract fn
    //     signatures and bodies accurately, but still can't resolve traits or
    //     macros. Good middle ground for a v2.
    //
    // For now, the heuristic (crate LOC + distinct symbol count) catches the
    // obvious cases: tiny crates (<500 LOC) and single-function usage of
    // moderate crates (<2000 LOC, <=3 API items).
}

/// Known crate -> std replacement mappings.
const STD_REPLACEMENTS: &[(&str, &str)] = &[
    ("lazy_static", "std::sync::LazyLock (Rust 1.80+)"),
    ("once_cell", "std::sync::OnceLock / LazyLock (Rust 1.80+)"),
    (
        "matches",
        "the built-in matches!() macro (stable since 1.42)",
    ),
];

/// High C_ref threshold: if a direct dependency has this many references,
/// it's deeply integrated and suggesting removal/gating is not useful.
const DEEPLY_INTEGRATED_THRESHOLD: usize = 15;

/// All inputs needed to compute an `UpstreamTarget`.
pub struct ComputeTargetInput {
    pub intermediate_name: String,
    pub intermediate_version: String,
    pub heavy_name: String,
    pub heavy_version: String,
    pub w_transitive: usize,
    pub w_unique: usize,
    pub scan_result: ScanResult,
    pub edge_meta: EdgeMeta,
    pub dep_chain: Vec<String>,
    pub was_renamed: bool,
    pub required_by_sibling: Option<String>,
    pub phantom: bool,
    pub intermediate_is_workspace_member: bool,
    /// True if the intermediate is a workspace member that no other workspace
    /// member depends on — it's already an opt-in integration crate.
    pub is_standalone_integration: bool,
    pub heavy_dep_loc: usize,
    pub heavy_dep_own_deps: usize,
    pub has_re_export_all: bool,
    /// True if the heavy dependency is a proc-macro crate. Proc macros are
    /// invoked via attributes/derives whose names often differ from the crate
    /// name, so regex scanning cannot reliably detect their usage.
    pub is_proc_macro: bool,
}

/// Compute hURRS, confidence, and determine the removal strategy.
pub fn compute_target(input: ComputeTargetInput) -> UpstreamTarget {
    let c_ref = input.scan_result.ref_count;
    let api_items_used = input.scan_result.distinct_items.len();

    let hurrs = if c_ref == 0 {
        None
    } else {
        Some(input.w_transitive as f64 / c_ref as f64)
    };

    let confidence = compute_confidence(&input, c_ref);

    let suggestion = compute_suggestion(&input, c_ref, api_items_used);

    UpstreamTarget {
        intermediate: PackageInfo {
            name: input.intermediate_name,
            version: input.intermediate_version,
        },
        heavy_dependency: PackageInfo {
            name: input.heavy_name,
            version: input.heavy_version,
        },
        w_transitive: input.w_transitive,
        w_unique: input.w_unique,
        c_ref,
        hurrs,
        confidence,
        scan_result: input.scan_result,
        suggestion,
        edge_meta: input.edge_meta,
        dep_chain: input.dep_chain,
        required_by_sibling: input.required_by_sibling,
        phantom: input.phantom,
        intermediate_is_workspace_member: input.intermediate_is_workspace_member,
        is_standalone_integration: input.is_standalone_integration,
        heavy_dep_loc: input.heavy_dep_loc,
        heavy_dep_own_deps: input.heavy_dep_own_deps,
        has_re_export_all: input.has_re_export_all,
    }
}

/// Max LOC for a leaf dep to be considered inlinable.
const SMALL_CRATE_LOC: usize = 500;

fn compute_suggestion(
    input: &ComputeTargetInput,
    c_ref: usize,
    api_items_used: usize,
) -> RemovalStrategy {
    let heavy_name = &input.heavy_name;
    let intermediate_name = &input.intermediate_name;
    let edge_meta = &input.edge_meta;
    let required_by_sibling = &input.required_by_sibling;
    let heavy_dep_loc = input.heavy_dep_loc;
    let heavy_dep_own_deps = input.heavy_dep_own_deps;
    let has_re_export_all = input.has_re_export_all;
    // If a sibling dep transitively requires this, it can't be removed.
    if let Some(sibling) = required_by_sibling {
        return RemovalStrategy::RequiredBySibling {
            sibling: sibling.clone(),
        };
    }

    if edge_meta.already_optional && edge_meta.platform_conditional {
        return RemovalStrategy::AlreadyGated {
            detail: "optional + platform-conditional".to_string(),
            enabling_features: Vec::new(),
            recommended_defaults: None,
        };
    }
    if edge_meta.already_optional {
        return RemovalStrategy::AlreadyGated {
            detail: "already optional".to_string(),
            enabling_features: Vec::new(),
            recommended_defaults: None,
        };
    }

    // Fix I: Detect foo-core -> foo-sys wrapper pattern.
    // An FFI wrapper always needs its -sys crate — never suggest gating it.
    if is_ffi_wrapper_pair(intermediate_name, heavy_name) || has_re_export_all {
        return RemovalStrategy::FeatureGate;
    }

    if c_ref == 0 {
        return RemovalStrategy::Remove;
    }

    // If ALL references are inside #[cfg(test)] or test files, this dep
    // belongs in [dev-dependencies], not [dependencies].
    if input.scan_result.test_only_refs == c_ref && c_ref > 0 {
        return RemovalStrategy::MoveToDevDeps;
    }

    if let Some((_, replacement)) = STD_REPLACEMENTS
        .iter()
        .find(|(name, _)| *name == heavy_name)
    {
        return RemovalStrategy::ReplaceWithStd {
            suggestion: replacement.to_string(),
        };
    }

    // Fix A+D: Only suggest inlining for leaf deps (0 own transitive deps).
    // Crates with their own dep trees are too complex to inline — they pull
    // in data, FFI bindings, or other infrastructure.
    let is_leaf = heavy_dep_own_deps == 0;
    if is_leaf {
        let is_small = heavy_dep_loc > 0 && heavy_dep_loc <= SMALL_CRATE_LOC;
        if is_small {
            return RemovalStrategy::InlineUpstream {
                heavy_loc: heavy_dep_loc,
                api_items_used,
            };
        }
    }

    RemovalStrategy::FeatureGate
}

/// Fix I: Detect FFI wrapper pairs like "foo-core" -> "foo-sys" or "foo" -> "foo-sys".
fn is_ffi_wrapper_pair(intermediate: &str, heavy: &str) -> bool {
    if !heavy.ends_with("-sys") {
        return false;
    }
    let heavy_base = heavy.strip_suffix("-sys").unwrap_or(heavy);
    // foo -> foo-sys
    if intermediate == heavy_base {
        return true;
    }
    // foo-core -> foo-sys
    if let Some(int_base) = intermediate.strip_suffix("-core") {
        if int_base == heavy_base {
            return true;
        }
    }
    false
}

fn compute_confidence(input: &ComputeTargetInput, c_ref: usize) -> Confidence {
    let scan_result = &input.scan_result;
    let edge_meta = &input.edge_meta;
    let heavy_name = &input.heavy_name;
    let intermediate_name = &input.intermediate_name;
    let was_renamed = input.was_renamed;
    let required_by_sibling = &input.required_by_sibling;
    let phantom = input.phantom;
    let intermediate_is_workspace_member = input.intermediate_is_workspace_member;
    let has_re_export_all = input.has_re_export_all;
    // Phantom deps aren't compiled on this platform — noise.
    if phantom {
        return Confidence::Noise;
    }

    // If a sibling dep transitively requires this, it's not removable — noise.
    if required_by_sibling.is_some() {
        return Confidence::Noise;
    }

    // Already optional + platform-conditional = noise.
    if edge_meta.already_optional && edge_meta.platform_conditional {
        return Confidence::Noise;
    }

    // Fix I: FFI wrapper pairs (foo-core -> foo-sys) are structural — noise.
    if is_ffi_wrapper_pair(intermediate_name, heavy_name) {
        return Confidence::Noise;
    }

    // Fix J: If the intermediate re-exports the entire heavy dep API, they're
    // structurally coupled — demote to low.
    if has_re_export_all {
        return Confidence::Low;
    }

    // Workspace member's own optional dep: the author already knows about this.
    if intermediate_is_workspace_member && edge_meta.already_optional {
        return Confidence::Low;
    }

    // Deeply integrated dep in a workspace member: high C_ref means it's core.
    if intermediate_is_workspace_member && c_ref >= DEEPLY_INTEGRATED_THRESHOLD {
        return Confidence::Low;
    }

    // Standalone integration crate: no other workspace member depends on it,
    // so it's already effectively opt-in. Suggesting changes within it is low-value.
    if input.is_standalone_integration {
        return Confidence::Low;
    }

    // If we found refs, confidence is generally high.
    if c_ref > 0 {
        // Proc-macro crates: our c_ref is unreliable because we only catch
        // explicit `use crate::` references, not #[derive(..)] invocations.
        // A low c_ref doesn't mean the dep is lightly used.
        if input.is_proc_macro {
            return Confidence::Low;
        }
        // If all refs are in generated files, downgrade to medium.
        if scan_result.generated_file_refs == c_ref {
            return Confidence::Medium;
        }
        // Fix H: If refs are spread across many files relative to the
        // intermediate crate's total file count, it's deeply integrated.
        if scan_result.files_with_matches >= 4 {
            return Confidence::Medium;
        }
        return Confidence::High;
    }

    // C_ref == 0 from here.

    if edge_meta.build_only {
        return Confidence::Low;
    }

    if heavy_name.ends_with("-sys") {
        return Confidence::Low;
    }

    if edge_meta.platform_conditional {
        return Confidence::Low;
    }

    if was_renamed {
        return Confidence::Medium;
    }

    // Proc-macro crates are invoked via #[derive(..)] or #[attr] whose names
    // often don't match the crate name. Our regex scanner can't reliably detect
    // this usage, so 0 refs on a proc-macro crate is not trustworthy.
    if input.is_proc_macro {
        return Confidence::Low;
    }

    Confidence::High
}

/// Returns the effective hURRS as f64 (None = infinity).
fn hurrs_value(h: Option<f64>) -> f64 {
    h.unwrap_or(f64::INFINITY)
}

/// Filter by threshold and sort by actionability.
pub fn rank_targets(
    mut targets: Vec<UpstreamTarget>,
    threshold: f64,
    top_n: usize,
    include_noise: bool,
) -> Vec<UpstreamTarget> {
    targets.retain(|t| hurrs_value(t.hurrs) >= threshold);
    if !include_noise {
        targets.retain(|t| t.confidence != Confidence::Noise);
    }
    targets.sort_by(|a, b| {
        // Primary: whether this actually saves deps (w_unique > 0 first).
        let a_saves = a.w_unique > 0;
        let b_saves = b.w_unique > 0;
        let saves_cmp = b_saves.cmp(&a_saves);
        if saves_cmp != std::cmp::Ordering::Equal {
            return saves_cmp;
        }
        // Secondary: confidence descending (High first).
        let conf_cmp = b.confidence.cmp(&a.confidence);
        if conf_cmp != std::cmp::Ordering::Equal {
            return conf_cmp;
        }
        // Tertiary: W_uniq descending (actual savings).
        let uniq_cmp = b.w_unique.cmp(&a.w_unique);
        if uniq_cmp != std::cmp::Ordering::Equal {
            return uniq_cmp;
        }
        // Quaternary: hURRS descending.
        hurrs_value(b.hurrs)
            .partial_cmp(&hurrs_value(a.hurrs))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    targets.truncate(top_n);
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EdgeMeta;
    use crate::scanner::ScanResult;

    fn base_input() -> ComputeTargetInput {
        ComputeTargetInput {
            intermediate_name: "some-crate".into(),
            intermediate_version: "1.0.0".into(),
            heavy_name: "heavy-dep".into(),
            heavy_version: "2.0.0".into(),
            w_transitive: 20,
            w_unique: 10,
            scan_result: ScanResult {
                heavy_crate_name: "heavy-dep".into(),
                searched_names: vec!["heavy_dep".into()],
                ref_count: 0,
                file_matches: vec![],
                files_with_matches: 0,
                generated_file_refs: 0,
                test_only_refs: 0,
                distinct_items: vec![],
                has_re_export_all: false,
            },
            edge_meta: EdgeMeta {
                build_only: false,
                already_optional: false,
                platform_conditional: false,
            },
            dep_chain: vec![],
            was_renamed: false,
            required_by_sibling: None,
            phantom: false,
            intermediate_is_workspace_member: false,
            is_standalone_integration: false,
            heavy_dep_loc: 0,
            heavy_dep_own_deps: 5,
            has_re_export_all: false,
            is_proc_macro: false,
        }
    }

    // -- Confidence properties --

    #[test]
    fn phantom_deps_are_noise() {
        let mut input = base_input();
        input.phantom = true;
        let t = compute_target(input);
        assert_eq!(t.confidence, Confidence::Noise);
    }

    #[test]
    fn sibling_required_deps_are_noise() {
        let mut input = base_input();
        input.required_by_sibling = Some("sibling-crate".into());
        let t = compute_target(input);
        assert_eq!(t.confidence, Confidence::Noise);
    }

    #[test]
    fn zero_refs_no_special_flags_is_high_confidence() {
        let input = base_input(); // c_ref = 0, no flags
        let t = compute_target(input);
        assert_eq!(t.confidence, Confidence::High);
    }

    #[test]
    fn proc_macro_with_zero_refs_is_low() {
        let mut input = base_input(); // c_ref = 0
        input.is_proc_macro = true;
        let t = compute_target(input);
        assert_eq!(t.confidence, Confidence::Low);
    }

    #[test]
    fn build_only_with_zero_refs_is_low() {
        let mut input = base_input();
        input.edge_meta.build_only = true;
        let t = compute_target(input);
        assert_eq!(t.confidence, Confidence::Low);
    }

    #[test]
    fn sys_crate_with_zero_refs_is_low() {
        let mut input = base_input();
        input.heavy_name = "openssl-sys".into();
        let t = compute_target(input);
        assert_eq!(t.confidence, Confidence::Low);
    }

    #[test]
    fn renamed_dep_with_zero_refs_is_medium() {
        let mut input = base_input();
        input.was_renamed = true;
        let t = compute_target(input);
        assert_eq!(t.confidence, Confidence::Medium);
    }

    // -- Suggestion properties --

    #[test]
    fn unused_dep_suggests_remove() {
        let input = base_input(); // c_ref = 0
        let t = compute_target(input);
        assert!(matches!(t.suggestion, RemovalStrategy::Remove));
    }

    #[test]
    fn sibling_required_blocks_removal() {
        let mut input = base_input();
        input.required_by_sibling = Some("other".into());
        let t = compute_target(input);
        assert!(matches!(
            t.suggestion,
            RemovalStrategy::RequiredBySibling { .. }
        ));
    }

    #[test]
    fn already_optional_suggests_already_gated() {
        let mut input = base_input();
        input.edge_meta.already_optional = true;
        input.scan_result.ref_count = 3;
        let t = compute_target(input);
        assert!(matches!(t.suggestion, RemovalStrategy::AlreadyGated { .. }));
    }

    #[test]
    fn known_std_replacement_suggests_replace() {
        let mut input = base_input();
        input.heavy_name = "lazy_static".into();
        input.scan_result.ref_count = 2;
        let t = compute_target(input);
        assert!(matches!(
            t.suggestion,
            RemovalStrategy::ReplaceWithStd { .. }
        ));
    }

    #[test]
    fn small_leaf_dep_suggests_inline() {
        let mut input = base_input();
        input.scan_result.ref_count = 1;
        input.heavy_dep_loc = 100;
        input.heavy_dep_own_deps = 0; // leaf
        let t = compute_target(input);
        assert!(matches!(
            t.suggestion,
            RemovalStrategy::InlineUpstream { .. }
        ));
    }

    #[test]
    fn non_leaf_dep_does_not_suggest_inline() {
        let mut input = base_input();
        input.scan_result.ref_count = 1;
        input.heavy_dep_loc = 100;
        input.heavy_dep_own_deps = 3; // not a leaf
        let t = compute_target(input);
        assert!(matches!(t.suggestion, RemovalStrategy::FeatureGate));
    }

    #[test]
    fn ffi_wrapper_pair_is_noise() {
        let mut input = base_input();
        input.intermediate_name = "openssl".into();
        input.heavy_name = "openssl-sys".into();
        let t = compute_target(input);
        assert_eq!(t.confidence, Confidence::Noise);
    }

    // -- hURRS properties --

    #[test]
    fn zero_refs_gives_infinite_hurrs() {
        let input = base_input();
        let t = compute_target(input);
        assert!(t.hurrs.is_none()); // None = infinity
    }

    #[test]
    fn hurrs_is_w_transitive_over_c_ref() {
        let mut input = base_input();
        input.scan_result.ref_count = 4;
        input.w_transitive = 20;
        let t = compute_target(input);
        assert_eq!(t.hurrs, Some(5.0));
    }

    // -- Ranking properties --

    #[test]
    fn targets_with_real_savings_rank_above_cosmetic() {
        let mut a = base_input();
        a.w_unique = 0; // cosmetic
        a.scan_result.ref_count = 0;

        let mut b = base_input();
        b.w_unique = 5; // real savings
        b.scan_result.ref_count = 0;

        let ranked = rank_targets(vec![compute_target(a), compute_target(b)], 0.0, 10, true);
        assert!(ranked[0].w_unique > 0);
    }

    #[test]
    fn threshold_filters_low_scoring_targets() {
        let mut input = base_input();
        input.scan_result.ref_count = 10;
        input.w_transitive = 20; // hURRS = 2.0

        let ranked = rank_targets(vec![compute_target(input)], 3.0, 10, true);
        assert!(ranked.is_empty());
    }

    #[test]
    fn test_only_refs_suggests_move_to_dev_deps() {
        let mut input = base_input();
        input.scan_result.ref_count = 3;
        input.scan_result.test_only_refs = 3; // ALL refs are in test code
        let t = compute_target(input);
        assert!(matches!(t.suggestion, RemovalStrategy::MoveToDevDeps));
    }

    #[test]
    fn mixed_test_and_prod_refs_does_not_suggest_dev_deps() {
        let mut input = base_input();
        input.scan_result.ref_count = 5;
        input.scan_result.test_only_refs = 2; // Only 2 of 5 are test
        let t = compute_target(input);
        assert!(!matches!(t.suggestion, RemovalStrategy::MoveToDevDeps));
    }

    #[test]
    fn standalone_integration_crate_is_low_confidence() {
        let mut input = base_input();
        input.scan_result.ref_count = 3;
        input.intermediate_is_workspace_member = true;
        input.is_standalone_integration = true;
        let t = compute_target(input);
        assert_eq!(t.confidence, Confidence::Low);
    }

    // -- Noise filtering --

    #[test]
    fn noise_targets_filtered_by_default() {
        let mut input = base_input();
        input.phantom = true; // -> Confidence::Noise
        let ranked = rank_targets(vec![compute_target(input)], 0.0, 10, false);
        assert!(ranked.is_empty());
    }

    #[test]
    fn noise_targets_included_when_requested() {
        let mut input = base_input();
        input.phantom = true; // -> Confidence::Noise
        let ranked = rank_targets(vec![compute_target(input)], 0.0, 10, true);
        assert_eq!(ranked.len(), 1);
    }
}
