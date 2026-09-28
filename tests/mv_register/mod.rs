//! MVR example and black-box tests of its document API.

mod implementation;
pub use implementation::*;

// Siblings of the implementation: these tests cannot access its private graph.
mod materialization;
mod revocation;
mod signatures;
mod tests;
