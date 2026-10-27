use std::io::IsTerminal;
use std::path::PathBuf;

use clap::Parser;
use kembar::config::Config;
use kembar::scan::{ScanOptions, run_blocking};
use kembar::select::KeepStrategy;
use kembar::util::format_size;

/// Terminal duplicate finder with Smart Selection.
#[derive(Parser, Debug)]
#[command(name = "kembar", version, about, long_about = None)]
struct Cli {
    /// Directories to scan (default: current directory).
    #[arg(value_name = "PATH")]
    paths: Vec<PathBuf>,

    /// Ignore files smaller than this, e.g. 1, 64K, 1M.
    #[arg(short = 'm', long, value_parser = parse_size)]
    min_size: Option<u64>,

    /// Only consider these extensions (comma separated), e.g. jpg,png.
    #[arg(short = 'x', long, value_delimiter = ',')]
    ext: Vec<String>,

    /// Extra exclude glob, repeatable, e.g. "*/.build/*".
    #[arg(short = 'e', long = "exclude")]
    exclude: Vec<String>,

    /// Which copy Smart Selection keeps.
    #[arg(short = 'k', long, value_enum)]
    keep: Option<KeepStrategy>,

    /// Reuse the on-disk hash cache between runs.
    #[arg(long)]
    cache: bool,

    /// Print a JSON report instead of opening the TUI.
    #[arg(long)]
    json: bool,

    /// Print a human readable report instead of opening the TUI.
    #[arg(long)]
    report: bool,

    /// Write an example config file and exit.
    #[arg(long)]
    init_config: bool,
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let config = Config::load();

    if cli.init_config {
        let path = config.write_example()?;
        println!("config: {}", path.display());
        return Ok(());
    }

    let raw_roots = if cli.paths.is_empty() {
        vec![std::env::current_dir()?]
    } else {
        cli.paths.clone()
    };
    let roots: Vec<PathBuf> = raw_roots
        .into_iter()
        .map(|path| path.canonicalize().unwrap_or(path))
        .collect();

    if let Some(missing) = roots.iter().find(|path| !path.exists()) {
        eprintln!("kembar: no such path: {}", missing.display());
        std::process::exit(2);
    }

    let mut excludes = config.excluded.clone();
    excludes.extend(cli.exclude.iter().cloned());

    let opts = ScanOptions {
        roots,
        min_size: cli.min_size.unwrap_or(config.min_size).max(1),
        extensions: cli.ext.clone(),
        extra_excludes: excludes,
        use_cache: cli.cache,
    };
    let keep = cli.keep.unwrap_or(config.keep);

    // Piped output or explicit flags: no TUI.
    if cli.json || cli.report || !std::io::stdout().is_terminal() {
        let report = run_blocking(&opts);
        if cli.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
            );
        } else {
            print_report(&report);
        }
        return Ok(());
    }

    kembar::tui::run(opts, keep)
}

fn print_report(report: &kembar::model::ScanReport) {
    if report.groups.is_empty() {
        println!("No duplicates found.");
        for error in &report.errors {
            eprintln!("error: {error}");
        }
        return;
    }
    println!(
        "{} groups · {} duplicate files · {} freeable · {} ms",
        report.groups.len(),
        report.duplicate_files,
        format_size(report.reclaimable),
        report.elapsed_ms
    );
    for group in report.groups.iter().take(50) {
        println!(
            "\n{:>9}  {}×  {}",
            format_size(group.size),
            group.files.len(),
            group.files[0].path.display()
        );
        for file in group.files.iter().skip(1) {
            println!("{:>9}      {}", "", file.path.display());
        }
    }
    if report.groups.len() > 50 {
        println!("\n… {} more groups (use --json)", report.groups.len() - 50);
    }
    for warning in report.warnings.iter().take(5) {
        eprintln!("warning: {warning}");
    }
}

fn parse_size(raw: &str) -> Result<u64, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("empty size".to_string());
    }
    let split = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let value: f64 = number
        .parse()
        .map_err(|_| format!("not a number: {number}"))?;
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        other => return Err(format!("unknown unit: {other}")),
    };
    Ok((value * multiplier) as u64)
}
