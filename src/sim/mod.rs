//! Seeded simulation of a federation of projects, for benchmarks and demos.

pub mod bench;
pub mod rng;
pub mod world;

pub use bench::{Run, System, run, table};
pub use world::{SimPr, Truth, World, WorldConfig};
