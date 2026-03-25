use crate::graph::EdgeMeta;
use crate::scanner::ScanResult;
use serde::{Deserialize, Serialize};

/// The recommended action for an upstream target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RemovalStrategy {
    /// The fat dependency appears entirely unused (C_ref = 0).
    Remove,
    /// Put the fat dependency behind a Cargo feature flag.
    FeatureGate,
    /// The fat dependency can be replaced with std functionality.
    ReplaceWithStd { suggestion: String },
    /// A lighter alternative crate exists.
    ReplaceWithLighter { alternative: String },
    /// The dependency is already optional/gated in an upstream crate.
    AlreadyGated {
        detail: String,
        /// Feature names of the intermediate crate that enable this optional dep.
        /// Empty if we couldn't determine them.
        #[serde(default)]
        enabling_features: Vec<String>,
        /// If the enabling feature is part of default, these are the default
        /// features the user should keep (i.e. defaults minus the one pulling
        /// in the fat dep). None if not enabled via defaults.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recommended_defaults: Option<Vec<String>>,
    },
    /// The dependency is required by a sibling dep — cannot be removed.
    RequiredBySibling { sibling: String },
    /// The dependency is small or lightly used — propose inlining the used code
    /// into the intermediate crate to eliminate the dep entirely.
    InlineUpstream {
        /// LOC of the fat dependency crate.
        fat_loc: usize,
        /// Number of distinct API items used.
        api_items_used: usize,
    },
}

impl std::fmt::Display for RemovalStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Remove => write!(f, "REMOVE (appears unused)"),
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
                fat_loc,
                api_items_used,
            } => {
                write!(
                    f,
                    "INLINE ({fat_loc} LOC crate, {api_items_used} items used)"
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
    pub intermediate: PackageInfo,
    pub fat_dependency: PackageInfo,
    pub w_transitive: usize,
    /// How many deps would actually disappear if this edge were cut.
    pub w_unique: usize,
    pub c_ref: usize,
    /// None means infinity (C_ref = 0, dependency appears unused).
    pub hurrs: Option<f64>,
    pub confidence: Confidence,
    pub scan_result: ScanResult,
    pub suggestion: RemovalStrategy,
    pub edge_meta: EdgeMeta,
    /// Shortest dependency chain from workspace to the fat dependency.
    #[serde(default)]
    pub dep_chain: Vec<String>,
    /// If set, a sibling dependency of the intermediate crate transitively
    /// requires the fat dep — so removing it would break the build.
    #[serde(default)]
    pub required_by_sibling: Option<String>,
    /// True if the fat dependency is not in the real platform-resolved tree.
    #[serde(default)]
    pub phantom: bool,
    /// True if the intermediate crate is a workspace member (user's own crate).
    #[serde(default)]
    pub intermediate_is_workspace_member: bool,
    /// Lines of code in the fat dependency crate (0 if unknown).
    #[serde(default)]
    pub fat_dep_loc: usize,
    /// Number of direct dependencies the fat dep itself has.
    /// A fat dep with 0 own deps is a leaf — potentially inlinable.
    #[serde(default)]
    pub fat_dep_own_deps: usize,
    /// True if the intermediate crate has `pub use <fat_dep>::*` (full re-export).
    #[serde(default)]
    pub has_re_export_all: bool,
    // TODO: Future — deep usage analysis via reachable LOC
    //
    // Currently we estimate "light usage" by counting distinct API symbols
    // referenced at call sites (e.g., "uses Adapter, from_u8"). This is a
    // rough proxy — one symbol might fan out to thousands of LOC internally.
    //
    // The ideal approach: measure how much code inside the fat dep is actually
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
    pub fat_name: String,
    pub fat_version: String,
    pub w_transitive: usize,
    pub w_unique: usize,
    pub scan_result: ScanResult,
    pub edge_meta: EdgeMeta,
    pub dep_chain: Vec<String>,
    pub was_renamed: bool,
    pub required_by_sibling: Option<String>,
    pub phantom: bool,
    pub intermediate_is_workspace_member: bool,
    pub fat_dep_loc: usize,
    pub fat_dep_own_deps: usize,
    pub has_re_export_all: bool,
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

    let confidence = compute_confidence(
        c_ref,
        &input.scan_result,
        &input.edge_meta,
        &input.fat_name,
        &input.intermediate_name,
        input.was_renamed,
        &input.required_by_sibling,
        input.phantom,
        input.intermediate_is_workspace_member,
        input.has_re_export_all,
    );

    let suggestion = compute_suggestion(
        c_ref,
        &input.fat_name,
        &input.intermediate_name,
        &input.edge_meta,
        &input.required_by_sibling,
        input.fat_dep_loc,
        api_items_used,
        input.fat_dep_own_deps,
        input.has_re_export_all,
    );

    UpstreamTarget {
        intermediate: PackageInfo {
            name: input.intermediate_name,
            version: input.intermediate_version,
        },
        fat_dependency: PackageInfo {
            name: input.fat_name,
            version: input.fat_version,
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
        fat_dep_loc: input.fat_dep_loc,
        fat_dep_own_deps: input.fat_dep_own_deps,
        has_re_export_all: input.has_re_export_all,
    }
}

/// Max LOC for a leaf dep to be considered inlinable.
const SMALL_CRATE_LOC: usize = 500;

fn compute_suggestion(
    c_ref: usize,
    fat_name: &str,
    intermediate_name: &str,
    edge_meta: &EdgeMeta,
    required_by_sibling: &Option<String>,
    fat_dep_loc: usize,
    api_items_used: usize,
    fat_dep_own_deps: usize,
    has_re_export_all: bool,
) -> RemovalStrategy {
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
    if is_ffi_wrapper_pair(intermediate_name, fat_name) || has_re_export_all {
        return RemovalStrategy::FeatureGate;
    }

    if c_ref == 0 {
        return RemovalStrategy::Remove;
    }

    if let Some((_, replacement)) = STD_REPLACEMENTS.iter().find(|(name, _)| *name == fat_name) {
        return RemovalStrategy::ReplaceWithStd {
            suggestion: replacement.to_string(),
        };
    }

    // Fix A+D: Only suggest inlining for leaf deps (0 own transitive deps).
    // Crates with their own dep trees are too complex to inline — they pull
    // in data, FFI bindings, or other infrastructure.
    let is_leaf = fat_dep_own_deps == 0;
    if is_leaf {
        let is_small = fat_dep_loc > 0 && fat_dep_loc <= SMALL_CRATE_LOC;
        if is_small {
            return RemovalStrategy::InlineUpstream {
                fat_loc: fat_dep_loc,
                api_items_used,
            };
        }
    }

    RemovalStrategy::FeatureGate
}

/// Fix I: Detect FFI wrapper pairs like "foo-core" -> "foo-sys" or "foo" -> "foo-sys".
fn is_ffi_wrapper_pair(intermediate: &str, fat: &str) -> bool {
    if !fat.ends_with("-sys") {
        return false;
    }
    let fat_base = fat.strip_suffix("-sys").unwrap_or(fat);
    // foo -> foo-sys
    if intermediate == fat_base {
        return true;
    }
    // foo-core -> foo-sys
    if let Some(int_base) = intermediate.strip_suffix("-core") {
        if int_base == fat_base {
            return true;
        }
    }
    false
}

fn compute_confidence(
    c_ref: usize,
    scan_result: &ScanResult,
    edge_meta: &EdgeMeta,
    fat_name: &str,
    intermediate_name: &str,
    was_renamed: bool,
    required_by_sibling: &Option<String>,
    phantom: bool,
    intermediate_is_workspace_member: bool,
    has_re_export_all: bool,
) -> Confidence {
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
    if is_ffi_wrapper_pair(intermediate_name, fat_name) {
        return Confidence::Noise;
    }

    // Fix J: If the intermediate re-exports the entire fat dep API, they're
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

    // If we found refs, confidence is generally high.
    if c_ref > 0 {
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

    if fat_name.ends_with("-sys") {
        return Confidence::Low;
    }

    if edge_meta.platform_conditional {
        return Confidence::Low;
    }

    if was_renamed {
        return Confidence::Medium;
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
) -> Vec<UpstreamTarget> {
    targets.retain(|t| hurrs_value(t.hurrs) >= threshold);
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
