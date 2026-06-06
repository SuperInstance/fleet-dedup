mod fingerprint;
mod group;
mod plan;
mod scanner;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "fleet-dedup", about = "Detect duplicate and near-duplicate repos in a fleet")]
struct Cli {
    /// Directory to scan for git repos
    scan_dir: PathBuf,

    /// Show what would happen without acting
    #[arg(long)]
    dry_run: bool,

    /// Maximum Hamming distance for near-duplicate detection (0-255)
    #[arg(long, default_value = "3")]
    max_distance: u32,

    /// Output file for JSON dedup plan (stdout if omitted)
    #[arg(long)]
    output: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    eprintln!("🔍 Scanning {} for git repos...", cli.scan_dir.display());
    let repos = scanner::scan_repos(&cli.scan_dir)?;
    eprintln!("   Found {} git repos", repos.len());

    if repos.is_empty() {
        eprintln!("No repos found. Exiting.");
        return Ok(());
    }

    eprintln!("🔑 Computing fingerprints...");
    let fingerprints = fingerprint::compute_all(&repos);

    eprintln!("📊 Comparing fingerprints...");
    let groups = group::group_repos(&fingerprints, cli.max_distance);

    eprintln!("📋 Generating dedup plan...");
    let dedup_plan = plan::generate_plan(&groups, &fingerprints);

    let json = serde_json::to_string_pretty(&dedup_plan)?;

    if let Some(ref path) = cli.output {
        std::fs::write(path, &json)?;
        eprintln!("✅ Plan written to {}", path.display());
    } else {
        println!("{}", json);
    }

    if cli.dry_run {
        eprintln!("🏁 Dry run — no actions taken.");
    }

    eprintln!(
        "📊 Summary: {} exact dupes, {} near-dupes, {} forks across {} repos",
        dedup_plan.summary.exact_duplicates,
        dedup_plan.summary.near_duplicates,
        dedup_plan.summary.forks,
        dedup_plan.summary.total_repos,
    );

    Ok(())
}
