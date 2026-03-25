use anyhow::{Context, Result};
use cargo_upstream_triage::cli::{AnalyzeArgs, Cli, Command, FlameArgs, OutputFormat, ReportArgs};
use cargo_upstream_triage::report::AnalysisReport;
use cargo_upstream_triage::{analyze, flamegraph, report};
use clap::Parser;
use std::io::Write;

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Flame(FlameArgs::default())) {
        Command::Analyze(args) => run_analyze_command(args),
        Command::Report(args) => run_report(args),
        Command::Flame(args) => run_flame(args),
    }
}

fn run_analyze_command(args: AnalyzeArgs) -> Result<()> {
    let output = args.output.clone();
    let format = args.format.clone();
    let verbose = args.verbose;

    let analysis = analyze::run_analyze(&args)?;

    let mut writer: Box<dyn Write> = open_writer(&output)?;
    write_output(&analysis, &format, verbose, &mut writer)?;

    // If writing to file, also save JSON alongside for the report subcommand.
    if let Some(path) = &output {
        let json_path = path.with_extension("json");
        if json_path != *path {
            let file = std::fs::File::create(&json_path)?;
            let mut json_writer = std::io::BufWriter::new(file);
            report::render_json(&analysis, &mut json_writer)?;
            eprintln!("JSON report saved to: {}", json_path.display());
        }
    }

    Ok(())
}

fn run_flame(args: FlameArgs) -> Result<()> {
    let analyze_args = AnalyzeArgs {
        manifest_path: args.manifest_path,
        threshold: args.threshold,
        top: args.top,
        fat_threshold: args.fat_threshold,
        format: OutputFormat::Html,
        output: None,
        verbose: args.verbose,
    };

    // Create a named temp file for the HTML output.
    // Use keep() so the file persists for the browser to read.
    let tmp_file = tempfile::Builder::new()
        .prefix("upstream-triage-")
        .suffix(".html")
        .tempfile()
        .context("failed to create temp file")?;
    let html_path = tmp_file
        .into_temp_path()
        .keep()
        .context("failed to persist temp file")?;

    let analysis = analyze::run_analyze(&analyze_args)?;

    let mut writer: Box<dyn Write> = {
        let file = std::fs::File::create(&html_path)
            .with_context(|| format!("failed to create output file: {}", html_path.display()))?;
        Box::new(std::io::BufWriter::new(file))
    };
    write_output(&analysis, &OutputFormat::Html, false, &mut writer)?;

    // Also save JSON alongside for convenience.
    let json_path = html_path.with_extension("json");
    let file = std::fs::File::create(&json_path)?;
    let mut json_writer = std::io::BufWriter::new(file);
    report::render_json(&analysis, &mut json_writer)?;
    eprintln!("JSON report saved to: {}", json_path.display());

    // Open the HTML in the user's default browser.
    let uri = format!("file://{}", html_path.display());
    eprintln!("Opening report: {}", uri);
    open::that(&uri).with_context(|| format!("failed to open browser for {}", uri))?;

    Ok(())
}

fn run_report(args: ReportArgs) -> Result<()> {
    let content = std::fs::read_to_string(&args.input)
        .with_context(|| format!("failed to read {}", args.input.display()))?;
    let analysis: AnalysisReport =
        serde_json::from_str(&content).context("failed to parse JSON report")?;

    let mut writer: Box<dyn Write> = open_writer(&args.output)?;
    write_output(&analysis, &args.format, args.verbose, &mut writer)?;

    Ok(())
}

/// Open a writer: file if path is given, stdout otherwise.
fn open_writer(output: &Option<std::path::PathBuf>) -> Result<Box<dyn Write>> {
    match output {
        Some(path) => {
            let file = std::fs::File::create(path)
                .with_context(|| format!("failed to create output file: {}", path.display()))?;
            Ok(Box::new(std::io::BufWriter::new(file)))
        }
        None => Ok(Box::new(std::io::stdout().lock())),
    }
}

/// Dispatch output rendering based on the chosen format.
fn write_output(
    analysis: &AnalysisReport,
    format: &OutputFormat,
    verbose: bool,
    writer: &mut dyn Write,
) -> Result<()> {
    match format {
        OutputFormat::Json => report::render_json(analysis, writer)?,
        OutputFormat::Text => report::render_text(analysis, writer, verbose)?,
        OutputFormat::Svg => {
            let tree = analysis.dep_tree.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "This report has no dep_tree data. \
                     Re-run `analyze` to generate a report with tree data for SVG rendering."
                )
            })?;
            flamegraph::render_flamegraph(tree, analysis.total_dependencies, writer)?;
        }
        OutputFormat::Html => {
            cargo_upstream_triage::html_report::render_html_report(analysis, writer)?;
        }
    }
    Ok(())
}
