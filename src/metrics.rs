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
    pub c_ref: usize,
    /// None means infinity (C_ref = 0, dependency appears unused).
    pub hurrs: Option<f64>,
    pub scan_result: ScanResult,
    pub suggestion: RemovalStrategy,
}

/// Known crate → std replacement mappings.
const STD_REPLACEMENTS: &[(&str, &str)] = &[
    ("lazy_static", "std::sync::LazyLock (Rust 1.80+)"),
    ("once_cell", "std::sync::OnceLock / LazyLock (Rust 1.80+)"),
    (
        "matches",
        "the built-in matches!() macro (stable since 1.42)",
    ),
];

/// Compute hURRS and determine the removal strategy.
pub fn compute_target(
    intermediate_name: &str,
    intermediate_version: &str,
    fat_name: &str,
    fat_version: &str,
    w_transitive: usize,
    scan_result: ScanResult,
) -> UpstreamTarget {
    let c_ref = scan_result.ref_count;

    let hurrs = if c_ref == 0 {
        None // infinity — dependency appears unused
    } else {
        Some(w_transitive as f64 / c_ref as f64)
    };

    let suggestion = if c_ref == 0 {
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
    };

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
        c_ref,
        hurrs,
        scan_result,
        suggestion,
    }
}

/// Filter by threshold and sort by hURRS descending.
/// Returns the effective hURRS as f64 (None = infinity).
fn hurrs_value(h: Option<f64>) -> f64 {
    h.unwrap_or(f64::INFINITY)
}

pub fn rank_targets(mut targets: Vec<UpstreamTarget>, threshold: f64, top_n: usize) -> Vec<UpstreamTarget> {
    targets.retain(|t| hurrs_value(t.hurrs) >= threshold);
    targets.sort_by(|a, b| {
        hurrs_value(b.hurrs)
            .partial_cmp(&hurrs_value(a.hurrs))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    targets.truncate(top_n);
    targets
}
