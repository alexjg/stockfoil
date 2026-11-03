//! Entry point for all Stockfoil proofs, including the shared executable code.
extern crate alloc;
#[path = "../src/verified/clock.rs"]
pub mod clock;
pub mod graph;
pub mod walk;
pub mod visibility;
#[path = "../src/verified/events.rs"]
pub mod events;
pub mod materialization;
pub mod examples;
