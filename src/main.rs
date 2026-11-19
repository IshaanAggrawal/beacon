use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use beacon::ledger::vouch_td::{Handle, Standing, VouchList};

#[derive(Parser)]
#[command(name = "beacon", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Summarize a Vouch-format VOUCHED.td file.
    Inspect {
        path: PathBuf,
        /// Forge assumed for handles without a `platform:` prefix.
        #[arg(long, default_value = "github")]
        platform: String,
    },
    /// Look up one user in a VOUCHED.td file.
    ///
    /// Exit codes match `vouch check`: 0 vouched, 1 denounced, 2 unknown.
    Check {
        path: PathBuf,
        user: String,
        #[arg(long, default_value = "github")]
        platform: String,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(3)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.command {
        Command::Inspect { path, platform } => {
            let list = load(&path, &platform)?;
            let vouched = list.with_standing(Standing::Vouched).count();
            let denounced: Vec<_> = list.with_standing(Standing::Denounced).collect();
            let with_reason = denounced.iter().filter(|e| e.reason.is_some()).count();

            println!("{}", path.display());
            println!("  {vouched} people are vouched for.");
            println!(
                "  {} people are denounced ({with_reason} with a written reason).",
                denounced.len()
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Check {
            path,
            user,
            platform,
        } => {
            let list = load(&path, &platform)?;
            let handle = Handle::parse(&user, &platform);
            let code = match list.lookup(&handle) {
                Some(e) if e.standing == Standing::Vouched => {
                    println!("{handle} is vouched for (line {}).", e.line);
                    0
                }
                Some(e) => {
                    let why = e.reason.as_deref().unwrap_or("no reason given");
                    println!("{handle} is denounced (line {}): {why}.", e.line);
                    1
                }
                None => {
                    println!("{handle} is not on this list.");
                    2
                }
            };
            Ok(ExitCode::from(code))
        }
    }
}

fn load(path: &Path, platform: &str) -> Result<VouchList> {
    let src =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    VouchList::parse(&src, platform).with_context(|| format!("parsing {}", path.display()))
}
