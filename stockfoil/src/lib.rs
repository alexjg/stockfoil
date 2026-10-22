//! # Stockfoil
//!
//! Authenticated commit histories for CRDTs, independent of payload contents.
//!
//! This crate re-exports the [`stockfoil_core`] state machines and adds the
//! native Rust [`Storage`] and [`Codec`] traits and the synchronous [`Graph`]
//! wrapper.
mod blob_hash;
mod graph;

pub use blob_hash::BlobHash;
pub use graph::{
    Codec, DecodeProgress, Decoder, Driven, EncodeProgress, Encoder, Graph, MemoryStorage,
    PayloadLocation, Storage,
};
pub use stockfoil_core::*;
