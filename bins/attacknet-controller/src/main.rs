//! `attacknet-controller` — orchestrates the 4-validator attacknet lab.
//!
//! Manages validator lifecycle, injects faults, collects evidence,
//! and produces structured reports for the scenario matrix.

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::{info, warn};

mod config;
mod evidence;
mod fault_injection;
mod lab;
mod metrics;
mod scenarios;
mod validators;

use config::LabConfig;
use lab::Lab;
use scenarios::{ScenarioRunner};

/// Lab identifier that must be present for any destructive operation
const LAB_CHAIN_ID: &str = "maya2c-attacknet-lab";
const LAB_ID: &str = "maya2c-attacknet-lab-v1";

#[derive(Parser, Debug)]
#[command(name = "attacknet-controller", version, about = "Maya2C Attacknet Lab Controller")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Lab configuration file
    #[arg(long, default_value = "D:\\Maya2C-attacknet-control\\lab.toml")]
    config: PathBuf,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Provision the lab (create directories, generate keys, write configs)
    Provision,
    /// Start all validators and infrastructure
    Start,
    /// Stop all lab processes cleanly
    Stop,
    /// Emergency termination of all lab processes
    Down,
    /// Run a specific scenario by ID
    Run {
        /// Scenario ID (e.g., "1.1", "2.3", "8.1")
        scenario_id: String,
        /// Number of rounds to repeat
        #[arg(long, default_value = "1")]
        rounds: usize,
    },
    /// Run all scenarios in a category
    RunCategory {
        /// Category number (1-8)
        category: u8,
        /// Number of rounds per scenario
        #[arg(long, default_value = "1")]
        rounds: usize,
    },
    /// Run the full 4-week rotation schedule
    RunSchedule {
        /// Week number (1-4)
        #[arg(long)]
        week: Option<u8>,
    },
    /// Show status of all lab components
    Status,
    /// Collect and display metrics
    Metrics,
    /// Generate daily summary report
    Report {
        /// Date (YYYY-MM-DD), defaults to today
        #[arg(long)]
        date: Option<String>,
    },
    /// Verify lab context (chain ID, data dirs, no production keys)
    VerifyContext,
    /// Clean up lab artifacts (logs, data, reports)
    Clean {
        /// Also remove generated keys and configs
        #[arg(long)]
        all: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    let level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(format!("attacknet_controller={level},maya2c_node=warn")))
        .init();

    // Verify lab context before any operation
    verify_lab_context(&cli.config)?;

    // Load or create lab configuration
    let config = LabConfig::load_or_create(&cli.config)?;

    // Create lab instance
    let lab = Lab::new(config).await?;

    match cli.command {
        Commands::Provision => {
            info!("Provisioning attacknet lab...");
            lab.provision().await?;
            info!("Lab provisioned successfully");
        }
        Commands::Start => {
            info!("Starting attacknet lab...");
            lab.start_all().await?;
            info!("Lab started successfully");
        }
        Commands::Stop => {
            info!("Stopping attacknet lab...");
            lab.stop_all().await?;
            info!("Lab stopped successfully");
        }
        Commands::Down => {
            warn!("Emergency termination of attacknet lab...");
            lab.emergency_down().await?;
            warn!("Lab terminated");
        }
        Commands::Run { scenario_id, rounds } => {
            info!("Running scenario {} ({} rounds)", scenario_id, rounds);
            let runner = ScenarioRunner::new(&lab);
            runner.run_scenario(&scenario_id, rounds).await?;
        }
        Commands::RunCategory { category, rounds } => {
            info!("Running category {} ({} rounds each)", category, rounds);
            let runner = ScenarioRunner::new(&lab);
            runner.run_category(category, rounds).await?;
        }
        Commands::RunSchedule { week } => {
            info!("Running attacknet schedule (week: {:?})", week);
            let runner = ScenarioRunner::new(&lab);
            runner.run_schedule(week).await?;
        }
        Commands::Status => {
            lab.status().await?;
        }
        Commands::Metrics => {
            lab.show_metrics().await?;
        }
        Commands::Report { date } => {
            lab.generate_report(date).await?;
        }
        Commands::VerifyContext => {
            verify_lab_context(&cli.config)?;
            println!("Lab context verified: chain_id={LAB_CHAIN_ID}, lab_id={LAB_ID}");
        }
        Commands::Clean { all } => {
            info!("Cleaning lab artifacts (all={})...", all);
            lab.clean(all).await?;
            info!("Clean complete");
        }
    }

    Ok(())
}

/// Verify we're operating within the authorized lab boundary
fn verify_lab_context(config_path: &Path) -> Result<()> {
    // Check chain ID environment variable
    let chain_id = std::env::var("MAYA2C_CHAIN_ID").unwrap_or_default();
    if chain_id != LAB_CHAIN_ID {
        // Allow if not set (will be set by lab config)
        if !chain_id.is_empty() {
            anyhow::bail!("CHAIN_ID mismatch: expected {}, got {}", LAB_CHAIN_ID, chain_id);
        }
    }

    // Check config file exists and has correct chain ID
    if config_path.exists() {
        let content = std::fs::read_to_string(config_path)?;
        let config: LabConfig = toml::from_str(&content)?;
        if config.chain_id != LAB_CHAIN_ID {
            anyhow::bail!("Config chain_id mismatch: expected {}, got {}", LAB_CHAIN_ID, config.chain_id);
        }
        if config.lab_id != LAB_ID {
            anyhow::bail!("Config lab_id mismatch: expected {}, got {}", LAB_ID, config.lab_id);
        }
    }

    // Verify no production keys in lab directories
    let base_dirs = [
        "D:\\Maya2C-attacknet-v1",
        "D:\\Maya2C-attacknet-v2",
        "D:\\Maya2C-attacknet-v3",
        "D:\\Maya2C-attacknet-v4",
        "D:\\Maya2C-attacknet-bootnode",
        "D:\\Maya2C-attacknet-adversary",
        "D:\\Maya2C-attacknet-genesis",
    ];

    for dir in &base_dirs {
        let path = Path::new(dir);
        if path.exists() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name.contains("prod") || name.contains("mainnet") || name.contains("real") {
                    anyhow::bail!("Production key detected in lab directory: {}", entry.path().display());
                }
            }
        }
    }

    Ok(())
}