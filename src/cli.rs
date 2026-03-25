use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "cargo-depflame",
    about = "Find high-ROI upstream dependency reduction opportunities",
    version
)]
pub struct Cli {
    /// When invoked as `cargo depflame`, cargo passes
    /// "depflame" as the first positional arg. We consume it here.
    #[arg(hide = true, required = false)]
    _subcommand_name: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Analyze the workspace dependency graph and generate a report.
    Analyze(AnalyzeArgs),
    /// Re-render a previously saved JSON analysis.
    Report(ReportArgs),
    /// Analyze and open an interactive HTML report in the default browser.
    Flame(FlameArgs),
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

/// Arguments shared between Analyze and Flame commands.
#[derive(Args, Debug)]
pub struct FlameArgs {
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

    /// Show detailed analysis (file matches, dep chains, metrics).
    #[arg(long, short)]
    pub verbose: bool,
}

impl Default for FlameArgs {
    fn default() -> Self {
        Self {
            manifest_path: PathBuf::from("Cargo.toml"),
            threshold: 3.0,
            top: 10,
            fat_threshold: 10,
            verbose: false,
        }
    }
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    /// Flamegraph / icicle-chart SVG showing dependency tree breakdown.
    Svg,
    /// Self-contained HTML report with flamegraph, targets table, and JSON.
    Html,
}
