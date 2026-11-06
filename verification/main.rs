//! Verify shared executable code and the explicitly conditional graph models.
#[path = "../stockfoil/src/verified/clock.rs"]
pub mod clock;
#[path = "../stockfoil/src/verified/events.rs"]
pub mod events;
pub mod coordinates;
pub mod graph;
pub mod walk;
pub mod authorization;
pub mod authorization_tests;
pub mod visibility;
pub mod examples;
pub mod intervals;
pub mod materialization;
#[path = "../stockfoil/src/verified/tree.rs"]
pub mod tree;
pub mod tree_examples;
pub mod tree_refinement;
#[path = "../stockfoil/src/verified/difference.rs"]
pub mod difference;
pub mod difference_examples;

// The original suite also runs without these modules, with --no-cheating.
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/stdlib.rs"]
pub mod stdlib;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/candidates.rs"]
pub mod candidates;
#[cfg(stockfoil_stdlib)]
pub mod candidate_refinement;
#[cfg(stockfoil_stdlib)]
pub mod candidate_examples;
#[path = "../stockfoil/src/verified/chain_tables.rs"]
pub mod chain_tables;
#[cfg(stockfoil_stdlib)]
pub mod chain_table_refinement;
