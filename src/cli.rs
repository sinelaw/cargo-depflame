use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cargo-upstream-triage",
    about = "Find high-ROI upstream dependency reduction opportunities",
    version
)]
pub struct Cli {
    /// When invoked as `cargo upstream-triage`, cargo passes
    /// "upstream-triage" as the first positional arg. We consume it here.
    #[arg(hide = true, required = false)]
    _subcommand_name: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Analyze the workspace dependency graph and generate a report.
    Analyze(AnalyzeArgs),
    /// Re-render a previously saved JSON analysis.
    Report(ReportArgs),
}

#[derive(Parser, Debug)]
pub struct AnalyzeArgs {
    /// Path to the workspace Cargo.toml.
    #[arg(long, default_value = "Cargo.toml")]
    pub manifest_path: PathBuf,

    /// Minimum hURRS score to include in the report.
    #[arg(long, default_value_t = 3.0)]
    pub threshold: f64,

    /// Show only the top N results.
    #[arg(long, default_value_t = 10)]
    pub top: usize,

    /// Minimum transitive weight for a node to be considered "fat".
    #[arg(long, default_value_t = 10)]
    pub fat_threshold: usize,

    /// Output format.
    #[arg(long, default_value = "text")]
    pub format: OutputFormat,

    /// Write report to a file instead of stdout.
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Show detailed analysis (file matches, dep chains, metrics).
    #[arg(long, short)]
    pub verbose: bool,

    /// Run deep call-graph analysis to measure reachable LOC in dependencies.
    /// Slow (minutes for large workspaces) but gives precise inline suggestions.
    #[arg(long)]
    pub deep_analysis: bool,
}

#[derive(Parser, Debug)]
pub struct ReportArgs {
    /// Path to a previously saved JSON analysis.
    #[arg(long)]
    pub input: PathBuf,

    /// Output format.
    #[arg(long, default_value = "text")]
    pub format: OutputFormat,

    /// Write report to a file instead of stdout.
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Show detailed analysis (file matches, dep chains, metrics).
    #[arg(long, short)]
    pub verbose: bool,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}
