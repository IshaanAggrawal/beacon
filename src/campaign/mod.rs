//! Cross-project campaign detection that never stores PR contents.

pub mod detector;
pub mod fingerprint;

pub use detector::{Cluster, Detector, DetectorConfig};
pub use fingerprint::{AuthorId, FileChange, Fingerprint, PrEvent, Salt};
