//! Cross-project campaign detection that never stores PR contents.

pub mod fingerprint;

pub use fingerprint::{AuthorId, FileChange, Fingerprint, PrEvent, Salt};
