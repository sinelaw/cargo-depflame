use anyhow::{Context, Result};
use cargo_depflame::cli::{AnalyzeArgs, Cli, Command, FlameArgs, OutputFormat, ReportArgs};
use cargo_depflame::report::AnalysisReport;
use cargo_depflame::{analyze, report};
use clap::Parser;
use std::io::Write;
use std::path::Path;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

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
    let verbose = args.common.verbose;

    let analysis = analyze::run_analyze(&args)?;

    let mut writer: Box<dyn Write> = open_writer(&output)?;
    write_output(&analysis, &format, verbose, &mut writer)?;

    // If writing to file, also save JSON alongside for the report subcommand.
    if let Some(path) = &output {
        let json_path = path.with_extension("json");
        if json_path != *path {
            save_json(&analysis, &json_path)?;
        }
    }

    Ok(())
}

fn run_flame(args: FlameArgs) -> Result<()> {
    let analyze_args = AnalyzeArgs {
        common: args.common,
        format: OutputFormat::Html,
        output: None,
    };

    // Create a temp file path for the HTML output.
    let html_path = std::env::temp_dir().join(format!(
        "depflame-{}-{}.html",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));

    let analysis = analyze::run_analyze(&analyze_args)?;

    let mut writer: Box<dyn Write> = {
        let file = std::fs::File::create(&html_path)
            .with_context(|| format!("failed to create output file: {}", html_path.display()))?;
        Box::new(std::io::BufWriter::new(file))
    };
    write_output(&analysis, &OutputFormat::Html, false, &mut writer)?;

    save_json(&analysis, &html_path.with_extension("json"))?;

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

/// Save analysis as JSON to the given path.
fn save_json(analysis: &AnalysisReport, path: &Path) -> Result<()> {
    let file = std::fs::File::create(path)
        .with_context(|| format!("failed to create JSON file: {}", path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    report::render_json(analysis, &mut writer)?;
    eprintln!("JSON report saved to: {}", path.display());
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
        OutputFormat::Html => {
            cargo_depflame::html_report::render_html_report(analysis, writer)?;
        }
    }
    Ok(())
}
