//! `.trust/policy.toml`: who a project is, who may sign for it, where its
//! peers publish their ledgers, and how it wants trust computed.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::attestation::decode_verifying_key;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    pub project: ProjectInfo,
    #[serde(default)]
    pub maintainers: Vec<Maintainer>,
    #[serde(default)]
    pub peers: Vec<PeerSource>,
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub id: String,
    /// Forge assumed for handles written without a `platform:` prefix.
    #[serde(default = "default_forge")]
    pub forge: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Maintainer {
    pub name: String,
    /// Hex-encoded ed25519 public key.
    pub key: String,
}

/// Where to fetch a peer's `.trust/` directory from. How much we trust the
/// peer is a signed `trust_peer` attestation in our ledger, not config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerSource {
    pub id: String,
    /// Anything `git clone` accepts.
    pub url: String,
}

/// Knobs for the trust computation. Each project chooses its own; the
/// defaults are what the benchmark was tuned with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Multiplier applied for every hop past a direct peer.
    pub hop_decay: f64,
    /// Peers further away than this are ignored.
    pub max_hops: usize,
    /// How long until another project's denouncement counts for half.
    pub denounce_half_life_days: f64,
    /// Each bad call by a peer multiplies our trust in it by this.
    pub blame_factor: f64,
    /// How long until a peer's bad call is half forgiven.
    pub blame_half_life_days: f64,
    /// Accept unsigned entries that were imported from a Vouch file.
    pub accept_imported: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hop_decay: 0.5,
            max_hops: 4,
            denounce_half_life_days: 180.0,
            blame_factor: 0.7,
            blame_half_life_days: 90.0,
            accept_imported: true,
        }
    }
}

fn default_forge() -> String {
    "github".into()
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("can't read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("{path} isn't valid: {source}")]
    Parse {
        path: String,
        source: toml::de::Error,
    },
    #[error("maintainer {0} has a key that isn't a valid ed25519 public key")]
    BadKey(String),
}

impl Policy {
    pub fn new(id: &str, forge: &str) -> Self {
        Self {
            project: ProjectInfo {
                id: id.into(),
                forge: forge.into(),
            },
            maintainers: Vec::new(),
            peers: Vec::new(),
            settings: Settings::default(),
        }
    }

    pub fn parse(src: &str, path: &str) -> Result<Self, PolicyError> {
        let policy: Policy = toml::from_str(src).map_err(|source| PolicyError::Parse {
            path: path.into(),
            source,
        })?;
        for m in &policy.maintainers {
            if decode_verifying_key(&m.key).is_none() {
                return Err(PolicyError::BadKey(m.name.clone()));
            }
        }
        Ok(policy)
    }

    pub fn load(path: &Path) -> Result<Self, PolicyError> {
        let src = std::fs::read_to_string(path).map_err(|source| PolicyError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&src, &path.display().to_string())
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("policies always serialize")
    }

    pub fn maintainer_for_key(&self, hex_key: &str) -> Option<&Maintainer> {
        self.maintainers
            .iter()
            .find(|m| m.key.eq_ignore_ascii_case(hex_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_policy_gets_defaults() {
        let p = Policy::parse("[project]\nid = \"ghostty\"\n", "policy.toml").unwrap();
        assert_eq!(p.project.forge, "github");
        assert_eq!(p.settings, Settings::default());
    }

    #[test]
    fn round_trips_through_toml() {
        let mut p = Policy::new("ghostty", "github");
        p.peers.push(PeerSource {
            id: "zig".into(),
            url: "https://github.com/ziglang/zig".into(),
        });
        p.settings.hop_decay = 0.3;
        assert_eq!(Policy::parse(&p.to_toml(), "x").unwrap(), p);
    }

    #[test]
    fn rejects_garbage_keys() {
        let src = "[project]\nid = \"x\"\n[[maintainers]]\nname = \"m\"\nkey = \"abc\"\n";
        assert!(matches!(
            Policy::parse(src, "x"),
            Err(PolicyError::BadKey(name)) if name == "m"
        ));
    }
}
