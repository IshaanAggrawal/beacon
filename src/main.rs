use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Days, Utc};
use clap::{Args, Parser, Subcommand};
use ed25519_dalek::SigningKey;

use beacon::graph::TrustEngine;
use beacon::graph::trust::Polarity;
use beacon::ledger::attestation::generate_key;
use beacon::ledger::import::{import_vouch_list, parse_history};
use beacon::ledger::policy::{Maintainer, PeerSource};
use beacon::ledger::sync::{fetch_peer, load_network, peers_dir};
use beacon::ledger::vouch_td::{Handle, Standing, VouchList};
use beacon::ledger::{Attestation, Kind, LEDGER_FILE, Ledger, POLICY_FILE, Policy, TRUST_DIR};
use beacon::report;
use beacon::sim::{self, World, WorldConfig};
use beacon::triage::groq::Author;
use beacon::triage::{self, GroqConfig, Thresholds};

#[derive(Parser)]
#[command(name = "beacon", version, about)]
struct Cli {
    /// The project's repository root.
    #[arg(long, global = true, default_value = ".")]
    dir: PathBuf,
    /// Pretend it's this moment (RFC 3339). Handy for demos and reproducible runs.
    #[arg(long, global = true, hide = true)]
    at: Option<DateTime<Utc>>,
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
    /// Set up .trust/ for this project and create a signing key.
    Init {
        project: String,
        /// Your name as a maintainer; the key is saved as .trust/keys/<name>.key.
        #[arg(long)]
        maintainer: String,
        #[arg(long, default_value = "github")]
        forge: String,
    },
    /// Vouch for a user.
    Vouch {
        user: String,
        #[command(flatten)]
        note: Note,
        /// Make it provisional: expires after this many days and only counts here.
        #[arg(long, value_name = "DAYS")]
        provisional: Option<u64>,
        /// What a provisional vouch covers, e.g. "docs".
        #[arg(long, requires = "provisional")]
        scope: Option<String>,
    },
    /// Denounce a user.
    Denounce {
        user: String,
        #[command(flatten)]
        note: Note,
    },
    /// Withdraw a vouch.
    Revoke {
        user: String,
        #[command(flatten)]
        note: Note,
    },
    /// Record that a denounced user has asked to be reconsidered.
    Appeal {
        user: String,
        #[command(flatten)]
        note: Note,
    },
    /// Lift your denouncement of a user.
    Rehabilitate {
        user: String,
        #[command(flatten)]
        note: Note,
    },
    /// Say how much you trust another project's judgment (0 to 1; 0 removes it).
    Trust {
        project: String,
        weight: f64,
        /// Where its repo lives, so `sync` can fetch it.
        #[arg(long)]
        url: Option<String>,
    },
    /// Import a VOUCHED.td file into this project's ledger as unsigned entries.
    Import {
        path: PathBuf,
        /// history.jsonl with the file's commits, to recover dates and who decided.
        #[arg(long)]
        history: Option<PathBuf>,
    },
    /// Check the ledger's signatures and chain.
    Verify,
    /// Fetch the latest trust data from every peer.
    Sync,
    /// Show how much you trust each project you can reach.
    Peers,
    /// Explain what the projects you trust say about a user.
    Why { user: String },
    /// Decide on a user and explain the verdict.
    Verdict {
        user: String,
        /// Use the built-in template even if an LLM is configured.
        #[arg(long)]
        no_llm: bool,
    },
    /// Show evidence cards for a few representative PRs from the simulation.
    Sample {
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long)]
        no_llm: bool,
    },
    /// Replay a simulated federation through flat Vouch and through beacon.
    Bench {
        /// First seed to run.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// How many consecutive seeds to run.
        #[arg(long, default_value_t = 1)]
        seeds: u64,
        /// Make paraphrased spam vary its structure too (0 to 1).
        #[arg(long, default_value_t = 0.0)]
        jitter: f64,
    },
}

#[derive(Args)]
struct Note {
    /// Why. Shown to anyone reviewing the decision.
    #[arg(long)]
    reason: Option<String>,
    /// Signing key to use; defaults to the only key in .trust/keys/.
    #[arg(long)]
    key: Option<PathBuf>,
}

fn main() -> ExitCode {
    // Optional: GROQ_API_KEY and GROQ_MODEL for the LLM explainer.
    let _ = dotenvy::dotenv();
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(3)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let now = cli.at.unwrap_or_else(Utc::now);
    let dir = cli.dir.as_path();

    match cli.command {
        Command::Inspect { path, platform } => inspect(&path, &platform),
        Command::Check {
            path,
            user,
            platform,
        } => check_file(&path, &user, &platform),
        Command::Init {
            project,
            maintainer,
            forge,
        } => init(dir, &project, &maintainer, &forge),
        Command::Vouch {
            user,
            note,
            provisional,
            scope,
        } => {
            let kind = if provisional.is_some() {
                Kind::ProvisionalVouch
            } else {
                Kind::Vouch
            };
            attest(dir, now, kind, &user, &note, |a| {
                a.expires_at = provisional.and_then(|d| now.checked_add_days(Days::new(d)));
                a.scope = scope.clone();
            })
        }
        Command::Denounce { user, note } => attest(dir, now, Kind::Denounce, &user, &note, |_| {}),
        Command::Revoke { user, note } => attest(dir, now, Kind::Revoke, &user, &note, |_| {}),
        Command::Appeal { user, note } => attest(dir, now, Kind::Appeal, &user, &note, |_| {}),
        Command::Rehabilitate { user, note } => {
            attest(dir, now, Kind::Rehabilitate, &user, &note, |_| {})
        }
        Command::Trust {
            project,
            weight,
            url,
        } => trust(dir, now, &project, weight, url),
        Command::Import { path, history } => import(dir, now, &path, history.as_deref()),
        Command::Verify => verify(dir),
        Command::Sync => sync(dir),
        Command::Peers => {
            let (policy, net, _) = load(dir, now)?;
            let view =
                TrustEngine::new(&net, &policy.project.id, policy.settings.clone(), now).run();
            print!("{}", report::peers(&view));
            Ok(ExitCode::SUCCESS)
        }
        Command::Bench {
            seed,
            seeds,
            jitter,
        } => {
            for s in seed..seed + seeds {
                let world = World::generate(WorldConfig {
                    seed: s,
                    layout_jitter: jitter,
                    ..WorldConfig::default()
                });
                let started = std::time::Instant::now();
                let run = sim::run(&world);
                print!("{}", sim::table(&world, &run));
                print!(
                    "
{}",
                    sim::bench::false_blocks(&run)
                );
                println!(
                    "
Judged {} PRs in {:.0} ms.
",
                    world.prs.len(),
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Verdict { user, no_llm } => {
            let (policy, net, _) = load(dir, now)?;
            let handle = Handle::parse(&user, &policy.project.forge).to_string();
            let view =
                TrustEngine::new(&net, &policy.project.id, policy.settings.clone(), now).run();
            let evidence = triage::assess(&view, &handle, None, &Thresholds::default());
            print_verdict(&evidence, llm(no_llm).as_ref());
            Ok(ExitCode::SUCCESS)
        }
        Command::Sample { seed, no_llm } => {
            let world = World::generate(WorldConfig {
                seed,
                ..WorldConfig::default()
            });
            let run = sim::run(&world);
            let llm = llm(no_llm);
            for (title, idx) in sim::bench::samples(&world, &run) {
                println!("── {title} ──");
                print_verdict(&run.evidence[idx], llm.as_ref());
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Why { user } => {
            let (policy, net, _) = load(dir, now)?;
            let handle = Handle::parse(&user, &policy.project.forge).to_string();
            let view =
                TrustEngine::new(&net, &policy.project.id, policy.settings.clone(), now).run();
            println!("{handle}, as seen by {}:", policy.project.id);
            for polarity in [Polarity::For, Polarity::Against] {
                print!(
                    "{}",
                    report::support(&view, &view.support(&handle, polarity))
                );
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn llm(disabled: bool) -> Option<GroqConfig> {
    if disabled {
        None
    } else {
        GroqConfig::from_env()
    }
}

fn print_verdict(evidence: &triage::Evidence, llm: Option<&GroqConfig>) {
    print!("{}", triage::explain::card(evidence));
    let note = triage::groq::explain(evidence, llm);
    let by = match &note.author {
        Author::Llm { model } => format!("written by {model}"),
        Author::Template { why: None } => "built-in template".to_string(),
        Author::Template { why: Some(why) } => format!("built-in template; {why}"),
    };
    println!(
        "  Summary ({by}):
    {}
",
        note.text
    );
}

fn inspect(path: &Path, platform: &str) -> Result<ExitCode> {
    let list = load_vouch_file(path, platform)?;
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

fn check_file(path: &Path, user: &str, platform: &str) -> Result<ExitCode> {
    let list = load_vouch_file(path, platform)?;
    let handle = Handle::parse(user, platform);
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

fn load_vouch_file(path: &Path, platform: &str) -> Result<VouchList> {
    let src =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    VouchList::parse(&src, platform).with_context(|| format!("parsing {}", path.display()))
}

fn trust_path(dir: &Path, file: &str) -> PathBuf {
    dir.join(TRUST_DIR).join(file)
}

fn init(dir: &Path, project: &str, maintainer: &str, forge: &str) -> Result<ExitCode> {
    let policy_path = trust_path(dir, POLICY_FILE);
    if policy_path.exists() {
        bail!("{} already exists", policy_path.display());
    }
    let keys = dir.join(TRUST_DIR).join("keys");
    std::fs::create_dir_all(&keys)?;
    // Private keys and fetched peers never belong in the project's history.
    std::fs::write(dir.join(TRUST_DIR).join(".gitignore"), "keys/\npeers/\n")?;

    let key = generate_key();
    let key_path = keys.join(format!("{maintainer}.key"));
    write_secret(&key_path, &hex::encode(key.to_bytes()))?;

    let mut policy = Policy::new(project, forge);
    policy.maintainers.push(Maintainer {
        name: maintainer.into(),
        key: hex::encode(key.verifying_key().to_bytes()),
    });
    std::fs::write(&policy_path, policy.to_toml())?;

    println!("Set up {project} in {}.", dir.join(TRUST_DIR).display());
    println!("  Policy: {}", policy_path.display());
    println!(
        "  Your signing key: {} (keep it private; it's git-ignored)",
        key_path.display()
    );
    Ok(ExitCode::SUCCESS)
}

fn write_secret(path: &Path, contents: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        writeln!(f, "{contents}")?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, format!("{contents}\n"))?;
    Ok(())
}

fn load_key(dir: &Path, explicit: Option<&Path>) -> Result<SigningKey> {
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => {
            let keys = dir.join(TRUST_DIR).join("keys");
            let found: Vec<PathBuf> = std::fs::read_dir(&keys)
                .with_context(|| format!("no keys in {}; run `beacon init` first", keys.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x == "key"))
                .collect();
            match found.as_slice() {
                [one] => one.clone(),
                [] => bail!("no .key files in {}", keys.display()),
                _ => bail!("several keys in {}; pick one with --key", keys.display()),
            }
        }
    };
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let bytes: [u8; 32] = hex::decode(text.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .with_context(|| format!("{} isn't a valid key file", path.display()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

/// Verifies the ledger, then signs (if a key is given) and appends `entries`.
fn append(
    dir: &Path,
    entries: Vec<Attestation>,
    key: Option<&SigningKey>,
) -> Result<Vec<Attestation>> {
    let policy = Policy::load(&trust_path(dir, POLICY_FILE))?;
    let ledger_path = trust_path(dir, LEDGER_FILE);
    let mut ledger = Ledger::load(&ledger_path)?;
    ledger
        .verify(&policy)
        .context("refusing to append to a ledger that doesn't verify")?;
    if let Some(key) = key {
        let hex_key = hex::encode(key.verifying_key().to_bytes());
        if policy.maintainer_for_key(&hex_key).is_none() {
            bail!("that key isn't listed as a maintainer in policy.toml");
        }
    }
    let count = entries.len();
    let mut added = Vec::with_capacity(count);
    for a in entries {
        a.validate()?;
        added.push(ledger.push(a, key).clone());
    }
    ledger.append_tail(&ledger_path, count)?;
    Ok(added)
}

fn attest(
    dir: &Path,
    now: DateTime<Utc>,
    kind: Kind,
    user: &str,
    note: &Note,
    extra: impl FnOnce(&mut Attestation),
) -> Result<ExitCode> {
    let policy = Policy::load(&trust_path(dir, POLICY_FILE))?;
    let key = load_key(dir, note.key.as_deref())?;
    let subject = Handle::parse(user, &policy.project.forge).to_string();

    let mut a = Attestation::new(kind, &policy.project.id, &subject, now);
    a.reason = note.reason.clone();
    extra(&mut a);
    let entry = append(dir, vec![a], Some(&key))?.remove(0);

    let what = match kind {
        Kind::Vouch => "Vouched for",
        Kind::ProvisionalVouch => "Provisionally vouched for",
        Kind::Denounce => "Denounced",
        Kind::Revoke => "Withdrew your vouch for",
        Kind::Appeal => "Recorded an appeal from",
        Kind::Rehabilitate => "Lifted your denouncement of",
        Kind::TrustPeer => unreachable!(),
    };
    print!("{what} {subject}");
    if let Some(until) = entry.expires_at {
        print!(" until {}", until.format("%Y-%m-%d"));
    }
    println!(
        ". Signed and added to {}.",
        trust_path(dir, LEDGER_FILE).display()
    );
    Ok(ExitCode::SUCCESS)
}

fn trust(
    dir: &Path,
    now: DateTime<Utc>,
    project: &str,
    weight: f64,
    url: Option<String>,
) -> Result<ExitCode> {
    let policy_path = trust_path(dir, POLICY_FILE);
    let mut policy = Policy::load(&policy_path)?;
    let key = load_key(dir, None)?;

    let mut a = Attestation::new(Kind::TrustPeer, &policy.project.id, project, now);
    a.weight = Some(weight);
    append(dir, vec![a], Some(&key))?;

    if let Some(url) = url {
        policy.peers.retain(|p| p.id != project);
        policy.peers.push(PeerSource {
            id: project.into(),
            url,
        });
        std::fs::write(&policy_path, policy.to_toml())?;
    }
    if weight == 0.0 {
        println!("You no longer trust {project}.");
    } else {
        println!("You now trust {project} at {}.", report::pct(weight));
    }
    Ok(ExitCode::SUCCESS)
}

fn import(dir: &Path, now: DateTime<Utc>, path: &Path, history: Option<&Path>) -> Result<ExitCode> {
    let policy = Policy::load(&trust_path(dir, POLICY_FILE))?;
    let list = load_vouch_file(path, &policy.project.forge)?;
    let events = match history {
        Some(h) => parse_history(&std::fs::read_to_string(h)?, &policy.project.forge)
            .with_context(|| format!("parsing {}", h.display()))?,
        None => Vec::new(),
    };
    let fallback = events.first().map_or(now, |e| e.date);
    let source = path
        .file_name()
        .map_or("VOUCHED.td".into(), |n| n.to_string_lossy().to_string());
    let entries = import_vouch_list(&list, &events, &policy.project.id, &source, fallback);

    let dated = entries
        .iter()
        .filter(|a| {
            matches!(
                &a.origin,
                beacon::ledger::Origin::Imported { actor: Some(_), .. }
            )
        })
        .count();
    let count = entries.len();
    append(dir, entries, None)?;
    println!(
        "Imported {count} entries from {} as unsigned.",
        path.display()
    );
    if history.is_some() {
        println!("  {dated} of them are matched to the maintainer who made the call.");
    }
    Ok(ExitCode::SUCCESS)
}

fn verify(dir: &Path) -> Result<ExitCode> {
    let policy = Policy::load(&trust_path(dir, POLICY_FILE))?;
    let ledger = Ledger::load(&trust_path(dir, LEDGER_FILE))?;
    match ledger.verify(&policy) {
        Ok(r) => {
            println!(
                "{}: {} entries, chain intact. {} signed by listed maintainers, {} imported unsigned.",
                policy.project.id,
                ledger.entries.len(),
                r.signed,
                r.imported
            );
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            println!("{}: the ledger does NOT verify. {e}", policy.project.id);
            Ok(ExitCode::from(1))
        }
    }
}

fn sync(dir: &Path) -> Result<ExitCode> {
    let policy = Policy::load(&trust_path(dir, POLICY_FILE))?;
    if policy.peers.is_empty() {
        println!("No peers to fetch. Add one with `beacon trust <project> <weight> --url <repo>`.");
        return Ok(ExitCode::SUCCESS);
    }
    let cache = peers_dir(dir);
    let mut failed = 0;
    for peer in &policy.peers {
        match fetch_peer(peer, &cache) {
            Ok(_) => println!("  fetched {}", peer.id),
            Err(e) => {
                failed += 1;
                println!("  couldn't fetch {}: {e}", peer.id);
            }
        }
    }
    let (_, _, problems) = load_network(dir, Utc::now())?;
    for p in &problems {
        println!("  ignoring a peer: {p}");
    }
    Ok(if failed == 0 && problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn load(
    dir: &Path,
    now: DateTime<Utc>,
) -> Result<(
    Policy,
    beacon::graph::Network,
    Vec<beacon::ledger::sync::SyncError>,
)> {
    let (policy, net, problems) = load_network(dir, now)?;
    for p in &problems {
        eprintln!("warning: ignoring a peer: {p}");
    }
    Ok((policy, net, problems))
}
