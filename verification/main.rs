//! Verify shared executable code and the explicitly conditional graph models.
#[path = "../stockfoil/src/verified/clock.rs"]
pub mod clock;
#[path = "../stockfoil/src/verified/events.rs"]
pub mod events;
pub mod coordinates;
pub mod graph;
pub mod walk;
pub mod visibility;
pub mod examples;
pub mod intervals;
pub mod materialization;
