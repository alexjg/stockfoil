//! A runnable, Unix sparse-volume CRDT. The application owns all storage/I/O.
//!
//! Canonical writes map increasing block indices to complete 64 KiB blocks.
//! Per-block winners are causally maximal participating writes, with concurrent
//! ties broken by the largest hash. SQLite indexes metadata on disk; payloads
//! and wire items are copied only through caller-sized buffers.
//!
//! See the example README for the image/index/checkpoint recovery protocol.
mod format;
mod image;
mod index;
mod storage;
mod volume;

pub use format::{BLOCK_BYTES, BodyDecoder, FORMAT};
pub use image::{DiskImage, Fault};
pub use index::BlockIndex;
pub use storage::{Region, Spool};
pub use volume::Volume;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Core(#[from] stockfoil::Error),
    #[error("invalid volume data: {0}")]
    Invalid(&'static str),
    #[error("injected interruption: {0:?}")]
    Interrupted(Fault),
    #[error("I/O failed after ledger mutation; reopen the volume")]
    Poisoned,
}
pub type Result<T> = std::result::Result<T, Error>;

/// Explicit payload I/O allocations, reused across commits, receives and exports.
pub struct Buffers {
    pub read: Vec<u8>,
    pub decode: Vec<u8>,
    pub block: Box<[u8; BLOCK_BYTES]>,
}
impl Buffers {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.read.is_empty() || self.decode.is_empty() {
            return Err(Error::Invalid("empty I/O buffer"));
        }
        Ok(())
    }
    pub fn new(read: usize, decode: usize) -> Self {
        assert!(read > 0 && decode > 0);
        Self {
            read: vec![0; read],
            decode: vec![0; decode],
            block: Box::new([0; BLOCK_BYTES]),
        }
    }
}

pub(crate) fn db(path: &std::path::Path) -> Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(path)?;
    // Bound page caches; temporary block sets spill to disk, not process RAM.
    conn.execute_batch(
        "PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA;
        PRAGMA cache_size=-2048; PRAGMA temp_store=FILE; PRAGMA temp.cache_size=-1024; PRAGMA foreign_keys=ON;",
    )?;
    Ok(conn)
}
pub(crate) fn sync_dir(path: &std::path::Path) -> Result<()> {
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}
pub(crate) fn reply(error: stockfoil::ReplyError) -> Error {
    match error {
        stockfoil::ReplyError::Invalid(e) => e.into(),
        _ => Error::Invalid("unexpected admission reply"),
    }
}
