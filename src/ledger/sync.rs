//! Fetching peers' trust data and loading it alongside our own.
//!
//! Peers are plain Git repos. We shallow-clone each one into
//! `.trust/peers/<id>/` and read its `.trust/` directory. Repos that only
//! have a Vouch file are read too, as unsigned imports. Nothing is ever
//! pushed anywhere.

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, Utc};
use thiserror::Error;

use super::import::import_vouch_list;
use super::policy::{PeerSource, Policy, PolicyError};
use super::store::{Ledger, LedgerError};
use super::vouch_td::VouchList;
use super::{Attestation, LEDGER_FILE, POLICY_FILE, TRUST_DIR};
use crate::graph::Network;

pub const PEERS_DIR: &str = "peers";
const VOUCH_FILES: [&str; 2] = [".github/VOUCHED.td", "VOUCHED.td"];

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("'{0}' can't be used as a peer id; use letters, digits, '-', '_' or '.'")]
    BadPeerId(String),
    #[error("git {action} failed for '{peer}': {detail}")]
    Git {
        peer: String,
        action: &'static str,
        detail: String,
    },
    #[error("couldn't run git: {0}")]
    GitMissing(std::io::Error),
    #[error("{0} has neither .trust/policy.toml nor a VOUCHED.td file")]
    NothingToRead(String),
    #[error("{dir} says it is '{found}', but we know it as '{expected}'")]
    IdMismatch {
        dir: String,
        expected: String,
        found: String,
    },
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error("{dir}: {source}")]
    Ledger { dir: String, source: LedgerError },
    #[error("{0}")]
    Vouch(String),
}

/// What we read from one project directory.
#[derive(Debug, Clone)]
pub struct LoadedProject {
    pub id: String,
    pub attestations: Vec<Attestation>,
    /// True when it came from a Vouch file rather than a signed ledger.
    pub unsigned: bool,
}

/// Peer ids become directory names, so keep them boring.
fn check_peer_id(id: &str) -> Result<(), SyncError> {
    let ok = !id.is_empty()
        && !id.starts_with('.')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(SyncError::BadPeerId(id.to_string()))
    }
}

pub fn peers_dir(project_dir: &Path) -> PathBuf {
    project_dir.join(TRUST_DIR).join(PEERS_DIR)
}

fn git(peer: &str, action: &'static str, args: &[&str]) -> Result<(), SyncError> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(SyncError::GitMissing)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(SyncError::Git {
            peer: peer.to_string(),
            action,
            detail: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        })
    }
}

/// Clones or refreshes one peer into `cache/<id>` and returns that path.
pub fn fetch_peer(peer: &PeerSource, cache: &Path) -> Result<PathBuf, SyncError> {
    check_peer_id(&peer.id)?;
    let dir = cache.join(&peer.id);
    let dir_str = dir.to_string_lossy().to_string();

    if dir.join(".git").exists() {
        git(
            &peer.id,
            "fetch",
            &["-C", &dir_str, "fetch", "--depth", "1", "origin", "HEAD"],
        )?;
        git(
            &peer.id,
            "reset",
            &["-C", &dir_str, "reset", "--hard", "--quiet", "FETCH_HEAD"],
        )?;
    } else {
        std::fs::create_dir_all(cache).map_err(|e| SyncError::Git {
            peer: peer.id.clone(),
            action: "clone",
            detail: e.to_string(),
        })?;
        // `--` keeps a hostile URL from being read as a git option.
        git(
            &peer.id,
            "clone",
            &[
                "clone", "--quiet", "--depth", "1", "--", &peer.url, &dir_str,
            ],
        )?;
    }
    Ok(dir)
}

/// Reads a project directory: a signed `.trust/` ledger if there is one,
/// otherwise a Vouch file treated as unsigned imports.
///
/// `expected_id` is what *we* call this project. A peer can't rename itself
/// into another project's slot.
pub fn load_project(
    dir: &Path,
    expected_id: Option<&str>,
    fallback_date: DateTime<Utc>,
) -> Result<LoadedProject, SyncError> {
    let dir_str = dir.display().to_string();
    let policy_path = dir.join(TRUST_DIR).join(POLICY_FILE);

    if policy_path.exists() {
        let policy = Policy::load(&policy_path)?;
        if let Some(expected) = expected_id {
            if policy.project.id != expected {
                return Err(SyncError::IdMismatch {
                    dir: dir_str,
                    expected: expected.to_string(),
                    found: policy.project.id,
                });
            }
        }
        let ledger = Ledger::load(&dir.join(TRUST_DIR).join(LEDGER_FILE)).map_err(|source| {
            SyncError::Ledger {
                dir: dir_str.clone(),
                source,
            }
        })?;
        ledger.verify(&policy).map_err(|source| SyncError::Ledger {
            dir: dir_str.clone(),
            source,
        })?;
        return Ok(LoadedProject {
            id: policy.project.id,
            attestations: ledger.entries,
            unsigned: false,
        });
    }

    let id = expected_id.ok_or_else(|| SyncError::NothingToRead(dir_str.clone()))?;
    for name in VOUCH_FILES {
        let path = dir.join(name);
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let list = VouchList::parse(&src, "github")
            .map_err(|e| SyncError::Vouch(format!("{}: {e}", path.display())))?;
        let attestations = import_vouch_list(&list, &[], id, name, fallback_date);
        return Ok(LoadedProject {
            id: id.to_string(),
            attestations,
            unsigned: true,
        });
    }
    Err(SyncError::NothingToRead(dir_str))
}

/// Loads our own project plus every peer already fetched into `.trust/peers/`.
/// Peers that fail to load are skipped and reported, never half-trusted.
pub fn load_network(
    project_dir: &Path,
    now: DateTime<Utc>,
) -> Result<(Policy, Network, Vec<SyncError>), SyncError> {
    let policy = Policy::load(&project_dir.join(TRUST_DIR).join(POLICY_FILE))?;
    let me = load_project(project_dir, Some(&policy.project.id), now)?;

    let mut net = Network::new();
    net.insert(&me.id, &me.attestations);

    let mut problems = Vec::new();
    let cache = peers_dir(project_dir);
    for peer in &policy.peers {
        let dir = cache.join(&peer.id);
        if check_peer_id(&peer.id).is_err() || !dir.exists() {
            continue;
        }
        match load_project(&dir, Some(&peer.id), now) {
            Ok(p) => net.insert(&p.id, &p.attestations),
            Err(e) => problems.push(e),
        }
    }
    Ok((policy, net, problems))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_ids_cannot_escape_the_cache() {
        for bad in ["", "..", ".hidden", "a/b", "a\\b", "x y"] {
            assert!(check_peer_id(bad).is_err(), "{bad:?} should be rejected");
        }
        for good in ["zig", "ghostty-org.ghostty", "my_project"] {
            assert!(check_peer_id(good).is_ok());
        }
    }
}
