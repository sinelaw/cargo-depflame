/// Errors that can occur during analysis.
#[derive(Debug)]
pub enum TriageError {
    /// Failed to run cargo_metadata.
    Metadata(cargo_metadata::Error),
    /// No resolve graph in metadata (workspace has no dependencies).
    NoResolveGraph,
    /// JSON serialization/deserialization error.
    Json(serde_json::Error),
}

impl std::fmt::Display for TriageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Metadata(e) => write!(f, "cargo_metadata failed: {e}"),
            Self::NoResolveGraph => write!(f, "no dependency resolution graph found"),

            Self::Json(e) => write!(f, "JSON error: {e}"),
        }
    }
}

impl std::error::Error for TriageError {}

impl From<cargo_metadata::Error> for TriageError {
    fn from(e: cargo_metadata::Error) -> Self {
        Self::Metadata(e)
    }
}

impl From<serde_json::Error> for TriageError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
