//! Combining trust and behavior into a verdict, and explaining it.

pub mod evidence;
pub mod verdict;

pub use evidence::{Evidence, assess};
pub use verdict::{Thresholds, Tier, decide};
