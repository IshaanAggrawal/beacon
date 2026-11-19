//! Everything a project writes down about who it trusts: its policy, its
//! signed ledger, importers for existing Vouch files, and peer sync.

pub mod attestation;
pub mod import;
pub mod policy;
pub mod store;
pub mod sync;
pub mod vouch_td;

pub use attestation::{Attestation, Kind, Origin};
pub use policy::{Policy, Settings};
pub use store::Ledger;

/// Where a project keeps its trust data, relative to the repo root.
pub const TRUST_DIR: &str = ".trust";
pub const POLICY_FILE: &str = "policy.toml";
pub const LEDGER_FILE: &str = "ledger.jsonl";
