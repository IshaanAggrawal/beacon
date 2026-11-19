//! `.trust/ledger.jsonl`: an append-only, hash-chained list of attestations.
//!
//! Each line names the hash of the line before it, so dropping, reordering or
//! editing an entry breaks the chain even when the edit re-signs the entry.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use ed25519_dalek::SigningKey;
use thiserror::Error;

use super::attestation::{Attestation, AttestationError, Origin};
use super::policy::Policy;

#[derive(Debug, Default, Clone)]
pub struct Ledger {
    pub entries: Vec<Attestation>,
}

#[derive(Debug, Error)]
pub enum LedgerError {
    #[error("can't read or write the ledger: {0}")]
    Io(#[from] std::io::Error),
    #[error("line {line} isn't a valid attestation: {source}")]
    Parse {
        line: usize,
        source: serde_json::Error,
    },
    #[error(
        "line {line} doesn't follow from the line before it; an entry was removed, reordered or edited"
    )]
    BrokenChain { line: usize },
    #[error("line {line} is issued by '{found}', but this ledger belongs to '{expected}'")]
    WrongIssuer {
        line: usize,
        expected: String,
        found: String,
    },
    #[error("line {line} is signed by a key that isn't listed as a maintainer")]
    UnknownKey { line: usize },
    #[error("line {line} is unsigned, and this policy doesn't accept imported entries")]
    UnsignedNotAllowed { line: usize },
    #[error("line {line}: {source}")]
    Attestation {
        line: usize,
        source: AttestationError,
    },
}

/// What `verify` found, for a plain-English summary.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub signed: usize,
    pub imported: usize,
}

impl Ledger {
    pub fn parse(src: &str) -> Result<Self, LedgerError> {
        let mut entries = Vec::new();
        for (idx, line) in src.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry = serde_json::from_str(line).map_err(|source| LedgerError::Parse {
                line: idx + 1,
                source,
            })?;
            entries.push(entry);
        }
        Ok(Self { entries })
    }

    /// A missing file is an empty ledger: every project starts that way.
    pub fn load(path: &Path) -> Result<Self, LedgerError> {
        match std::fs::read_to_string(path) {
            Ok(src) => Self::parse(&src),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Links `entry` to the current tip, signs it if a key is given, and adds
    /// it. Linking comes first so the signature covers the link too.
    pub fn push(&mut self, mut entry: Attestation, key: Option<&SigningKey>) -> &Attestation {
        entry.prev = self.entries.last().map(Attestation::hash);
        if let Some(key) = key {
            entry.sign(key);
        }
        self.entries.push(entry);
        self.entries.last().expect("just pushed")
    }

    /// Appends the newest `count` entries to `path` without rewriting earlier lines.
    pub fn append_tail(&self, path: &Path, count: usize) -> Result<(), LedgerError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        for e in &self.entries[self.entries.len().saturating_sub(count)..] {
            writeln!(file, "{}", serde_json::to_string(e).expect("serializes"))?;
        }
        Ok(())
    }

    pub fn write_all(&self, path: &Path) -> Result<(), LedgerError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut out = String::new();
        for e in &self.entries {
            out.push_str(&serde_json::to_string(e).expect("serializes"));
            out.push('\n');
        }
        std::fs::write(path, out)?;
        Ok(())
    }

    /// Checks the chain, the issuer, every signature and every key against
    /// `policy`. Stops at the first problem: a ledger is trusted whole or not at all.
    pub fn verify(&self, policy: &Policy) -> Result<VerifyReport, LedgerError> {
        let mut report = VerifyReport::default();
        let mut prev: Option<String> = None;

        for (idx, entry) in self.entries.iter().enumerate() {
            let line = idx + 1;
            if entry.prev != prev {
                return Err(LedgerError::BrokenChain { line });
            }
            if entry.issuer != policy.project.id {
                return Err(LedgerError::WrongIssuer {
                    line,
                    expected: policy.project.id.clone(),
                    found: entry.issuer.clone(),
                });
            }
            entry
                .validate()
                .map_err(|source| LedgerError::Attestation { line, source })?;

            match &entry.origin {
                Origin::Signed { key } => {
                    entry
                        .verify_signature()
                        .map_err(|source| LedgerError::Attestation { line, source })?;
                    if policy.maintainer_for_key(key).is_none() {
                        return Err(LedgerError::UnknownKey { line });
                    }
                    report.signed += 1;
                }
                Origin::Imported { .. } if policy.settings.accept_imported => report.imported += 1,
                Origin::Imported { .. } => return Err(LedgerError::UnsignedNotAllowed { line }),
            }
            prev = Some(entry.hash());
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::attestation::Kind;
    use crate::ledger::attestation::tests::{at, key};
    use crate::ledger::policy::Maintainer;

    fn policy() -> Policy {
        let mut p = Policy::new("ghostty", "github");
        p.maintainers.push(Maintainer {
            name: "mitchellh".into(),
            key: hex::encode(key(1).verifying_key().to_bytes()),
        });
        p
    }

    fn sample() -> Ledger {
        let mut l = Ledger::default();
        for (kind, subject, day) in [
            (Kind::Vouch, "github:alice", 0),
            (Kind::Vouch, "github:bob", 1),
            (Kind::Denounce, "github:mallory", 2),
        ] {
            l.push(
                Attestation::new(kind, "ghostty", subject, at(day)),
                Some(&key(1)),
            );
        }
        l
    }

    #[test]
    fn a_clean_ledger_verifies() {
        let report = sample().verify(&policy()).unwrap();
        assert_eq!(
            report,
            VerifyReport {
                signed: 3,
                imported: 0
            }
        );
    }

    #[test]
    fn patching_the_chain_after_a_deletion_needs_a_maintainer_key() {
        // Drop entry 2 and re-point entry 3 at entry 1: the chain lines up
        // again, but the link was signed, so the signature no longer matches.
        let mut l = sample();
        l.entries.remove(1);
        l.entries[1].prev = Some(l.entries[0].hash());
        assert!(matches!(
            l.verify(&policy()),
            Err(LedgerError::Attestation { line: 2, .. })
        ));
    }

    #[test]
    fn removing_an_entry_breaks_the_chain() {
        let mut l = sample();
        l.entries.remove(1);
        assert!(matches!(
            l.verify(&policy()),
            Err(LedgerError::BrokenChain { line: 2 })
        ));
    }

    #[test]
    fn re_signing_an_edited_entry_still_breaks_the_chain() {
        let mut l = sample();
        l.entries[0].subject = "github:mallory".into();
        l.entries[0].sign(&key(1));
        assert!(matches!(
            l.verify(&policy()),
            Err(LedgerError::BrokenChain { line: 2 })
        ));
    }

    #[test]
    fn a_stranger_key_is_rejected() {
        let mut l = Ledger::default();
        let a = Attestation::new(Kind::Vouch, "ghostty", "github:alice", at(0));
        l.push(a, Some(&key(9)));
        assert!(matches!(
            l.verify(&policy()),
            Err(LedgerError::UnknownKey { line: 1 })
        ));
    }

    #[test]
    fn imported_entries_need_policy_consent() {
        let mut l = Ledger::default();
        l.push(
            Attestation::new(Kind::Vouch, "ghostty", "github:alice", at(0)),
            None,
        );
        let mut p = policy();
        assert_eq!(l.verify(&p).unwrap().imported, 1);
        p.settings.accept_imported = false;
        assert!(matches!(
            l.verify(&p),
            Err(LedgerError::UnsignedNotAllowed { line: 1 })
        ));
    }

    #[test]
    fn round_trips_through_jsonl() {
        let l = sample();
        let text: String = l
            .entries
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        let back = Ledger::parse(&text).unwrap();
        assert!(back.verify(&policy()).is_ok());
    }
}
