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
    AlreadyGated { detail: String },
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
            Self::AlreadyGated { detail } => {
                write!(f, "ALREADY GATED ({detail})")
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

/// Compute hURRS, confidence, and determine the removal strategy.
pub fn compute_target(
    intermediate_name: &str,
    intermediate_version: &str,
    fat_name: &str,
    fat_version: &str,
    w_transitive: usize,
    w_unique: usize,
    scan_result: ScanResult,
    edge_meta: EdgeMeta,
    dep_chain: Vec<String>,
    was_renamed: bool,
    required_by_sibling: Option<String>,
    phantom: bool,
    intermediate_is_workspace_member: bool,
    fat_dep_loc: usize,
) -> UpstreamTarget {
    let c_ref = scan_result.ref_count;
    let api_items_used = scan_result.distinct_items.len();

    let hurrs = if c_ref == 0 {
        None // infinity — dependency appears unused
    } else {
        Some(w_transitive as f64 / c_ref as f64)
    };

    // Determine confidence.
    let confidence = compute_confidence(
        c_ref,
        &scan_result,
        &edge_meta,
        fat_name,
        was_renamed,
        &required_by_sibling,
        phantom,
        intermediate_is_workspace_member,
    );

    // Determine suggestion.
    let suggestion = compute_suggestion(
        c_ref,
        fat_name,
        &edge_meta,
        &required_by_sibling,
        fat_dep_loc,
        api_items_used,
    );

    UpstreamTarget {
        intermediate: PackageInfo {
            name: intermediate_name.to_string(),
            version: intermediate_version.to_string(),
        },
        fat_dependency: PackageInfo {
            name: fat_name.to_string(),
            version: fat_version.to_string(),
        },
        w_transitive,
        w_unique,
        c_ref,
        hurrs,
        confidence,
        scan_result,
        suggestion,
        edge_meta,
        dep_chain,
        required_by_sibling,
        phantom,
        intermediate_is_workspace_member,
        fat_dep_loc,
    }
}

/// Thresholds for suggesting "inline the code instead of depending on it".
const SMALL_CRATE_LOC: usize = 500;
const LIGHT_USAGE_ITEMS: usize = 3;

fn compute_suggestion(
    c_ref: usize,
    fat_name: &str,
    edge_meta: &EdgeMeta,
    required_by_sibling: &Option<String>,
    fat_dep_loc: usize,
    api_items_used: usize,
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
        };
    }
    if edge_meta.already_optional {
        return RemovalStrategy::AlreadyGated {
            detail: "already optional".to_string(),
        };
    }

    if c_ref == 0 {
        return RemovalStrategy::Remove;
    }

    if let Some((_, replacement)) = STD_REPLACEMENTS
        .iter()
        .find(|(name, _)| *name == fat_name)
    {
        return RemovalStrategy::ReplaceWithStd {
            suggestion: replacement.to_string(),
        };
    }

    // Small crate: suggest inlining regardless of usage breadth.
    // Light usage of a moderate crate: suggest inlining the specific items.
    // But never suggest inlining large crates (>2000 LOC) — feature-gate instead.
    let is_small = fat_dep_loc > 0 && fat_dep_loc <= SMALL_CRATE_LOC;
    let is_light = api_items_used > 0
        && api_items_used <= LIGHT_USAGE_ITEMS
        && fat_dep_loc > 0
        && fat_dep_loc <= 2000;
    if is_small || is_light {
        return RemovalStrategy::InlineUpstream {
            fat_loc: fat_dep_loc,
            api_items_used,
        };
    }

    RemovalStrategy::FeatureGate
}

fn compute_confidence(
    c_ref: usize,
    scan_result: &ScanResult,
    edge_meta: &EdgeMeta,
    fat_name: &str,
    was_renamed: bool,
    required_by_sibling: &Option<String>,
    phantom: bool,
    intermediate_is_workspace_member: bool,
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

    // Workspace member's own optional dep: the author already knows about this.
    // Not useful to tell them "your dep is optional" — they declared it.
    if intermediate_is_workspace_member && edge_meta.already_optional {
        return Confidence::Low;
    }

    // Deeply integrated dep in a workspace member: high C_ref means it's core,
    // not something that can be realistically feature-gated.
    if intermediate_is_workspace_member && c_ref >= DEEPLY_INTEGRATED_THRESHOLD {
        return Confidence::Low;
    }

    // If we found refs, confidence is generally high.
    if c_ref > 0 {
        // If all refs are in generated files, downgrade to medium.
        if scan_result.generated_file_refs == c_ref {
            return Confidence::Medium;
        }
        return Confidence::High;
    }

    // C_ref == 0 from here. Assess why it might be a false positive.

    // Build-only deps won't appear in src — low confidence for "unused".
    if edge_meta.build_only {
        return Confidence::Low;
    }

    // -sys crates are typically FFI link deps, not referenced via `use`.
    if fat_name.ends_with("-sys") {
        return Confidence::Low;
    }

    // Platform-conditional deps may not be used on the current platform.
    if edge_meta.platform_conditional {
        return Confidence::Low;
    }

    // If the dep was renamed in Cargo.toml and we still found 0 refs,
    // we already searched under the alias, so this is more likely real.
    // But if we couldn't find the Cargo.toml (no rename info), medium.
    if was_renamed {
        return Confidence::Medium;
    }

    // Default: no refs found, no obvious explanation — high confidence unused.
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
        // Primary: confidence descending (High first).
        let conf_cmp = b.confidence.cmp(&a.confidence);
        if conf_cmp != std::cmp::Ordering::Equal {
            return conf_cmp;
        }
        // Secondary: prefer upstream (non-workspace) targets over workspace members.
        let upstream_a = !a.intermediate_is_workspace_member;
        let upstream_b = !b.intermediate_is_workspace_member;
        let upstream_cmp = upstream_b.cmp(&upstream_a);
        if upstream_cmp != std::cmp::Ordering::Equal {
            return upstream_cmp;
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
