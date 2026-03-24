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
    /// The dependency is already optional/gated — no upstream change needed.
    AlreadyGated { detail: String },
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
) -> UpstreamTarget {
    let c_ref = scan_result.ref_count;

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
    );

    // Determine suggestion.
    let suggestion = compute_suggestion(c_ref, fat_name, &edge_meta);

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
    }
}

fn compute_suggestion(c_ref: usize, fat_name: &str, edge_meta: &EdgeMeta) -> RemovalStrategy {
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
        RemovalStrategy::Remove
    } else if let Some((_, replacement)) = STD_REPLACEMENTS
        .iter()
        .find(|(name, _)| *name == fat_name)
    {
        RemovalStrategy::ReplaceWithStd {
            suggestion: replacement.to_string(),
        }
    } else {
        RemovalStrategy::FeatureGate
    }
}

fn compute_confidence(
    c_ref: usize,
    scan_result: &ScanResult,
    edge_meta: &EdgeMeta,
    fat_name: &str,
    was_renamed: bool,
) -> Confidence {
    // Already optional + platform-conditional = noise.
    if edge_meta.already_optional && edge_meta.platform_conditional {
        return Confidence::Noise;
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
        // We searched the alias and still found nothing — medium.
        return Confidence::Medium;
    }

    // Default: no refs found, no obvious explanation — high confidence unused.
    Confidence::High
}

/// Returns the effective hURRS as f64 (None = infinity).
fn hurrs_value(h: Option<f64>) -> f64 {
    h.unwrap_or(f64::INFINITY)
}

/// Filter by threshold and sort by hURRS descending.
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
        // Secondary: hURRS descending.
        hurrs_value(b.hurrs)
            .partial_cmp(&hurrs_value(a.hurrs))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    targets.truncate(top_n);
    targets
}
