//! Everything a project writes down about who it trusts.
//!
//! For now this only reads Vouch's `VOUCHED.td` format. Signed attestations
//! and peer sync land on top of it.

pub mod vouch_td;
