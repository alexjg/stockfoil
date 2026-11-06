use crate::{Error, Result, db};
use rusqlite::params;
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    os::unix::fs::FileExt,
    path::Path,
};
use stockfoil::PayloadId;

#[derive(Debug, Clone, Copy)]
pub struct Region {
    pub offset: u64,
    pub len: u64,
}
pub(crate) struct Staging {
    offset: u64,
    len: u64,
}

/// Append-only canonical bytes. Region metadata is commit-sized, not block-sized.
/// Catalog rows are committed only after their bytes have been synced.
pub struct Spool {
    file: File,
    pub(crate) catalog: rusqlite::Connection,
    regions: HashMap<PayloadId, Region>,
    pending: Vec<PayloadId>,
    tail: u64,
    _lock: File,
}
impl Spool {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("volume.lock"))?;
        lock.try_lock()
            .map_err(|_| Error::Invalid("volume already open"))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("payloads"))?;
        let catalog = db(&dir.join("catalog.sqlite"))?;
        catalog.execute_batch("CREATE TABLE IF NOT EXISTS regions(id INTEGER PRIMARY KEY, len INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS items(key TEXT PRIMARY KEY, head BLOB NOT NULL, item_id BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS members(key TEXT NOT NULL REFERENCES items(key) ON DELETE CASCADE,
                nth INTEGER NOT NULL, id INTEGER NOT NULL REFERENCES regions(id), PRIMARY KEY(key,nth));")?;
        let file_len = file.metadata()?.len();
        let mut regions = HashMap::new();
        {
            let mut stmt = catalog.prepare("SELECT id,len FROM regions")?;
            for row in stmt.query_map([], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?)))? {
                let (offset, len) = row?;
                if offset.checked_add(len).is_none_or(|end| end > file_len) {
                    return Err(Error::Invalid("truncated spool"));
                }
                regions.insert(offset, Region { offset, len });
            }
        }
        let tail = file.metadata()?.len(); // orphaned tails are never reused
        Ok(Self {
            file,
            catalog,
            regions,
            pending: Vec::new(),
            tail,
            _lock: lock,
        })
    }
    pub(crate) fn begin(&self) -> Staging {
        Staging {
            offset: self.tail,
            len: 0,
        }
    }
    pub(crate) fn append(&mut self, staging: &mut Staging, bytes: &[u8]) -> Result<()> {
        if staging.offset + staging.len != self.tail {
            return Err(Error::Invalid("interleaved spool staging"));
        }
        self.file.write_all_at(bytes, self.tail)?;
        staging.len = staging
            .len
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Invalid("payload too large"))?;
        self.tail = self
            .tail
            .checked_add(bytes.len() as u64)
            .ok_or(Error::Invalid("spool too large"))?;
        Ok(())
    }
    pub(crate) fn finish(&mut self, staging: Staging) -> Result<PayloadId> {
        // Empty malformed payloads still need distinct IDs.
        if staging.len == 0 {
            self.file.write_all_at(&[0], self.tail)?;
            self.tail += 1;
        }
        let id = staging.offset;
        self.regions.insert(
            id,
            Region {
                offset: id,
                len: staging.len,
            },
        );
        self.pending.push(id);
        Ok(id)
    }
    pub fn region(&self, id: PayloadId) -> Result<Region> {
        self.regions
            .get(&id)
            .copied()
            .ok_or(Error::Invalid("unknown payload ID"))
    }
    pub fn read(&self, region: Region, offset: u64, out: &mut [u8]) -> Result<()> {
        if offset
            .checked_add(out.len() as u64)
            .is_none_or(|end| end > region.len)
        {
            return Err(Error::Invalid("read beyond canonical payload"));
        }
        self.file.read_exact_at(out, region.offset + offset)?;
        Ok(())
    }
    pub fn sync(&mut self) -> Result<()> {
        self.file.sync_all()?;
        let tx = self.catalog.transaction()?;
        for id in &self.pending {
            tx.execute(
                "INSERT INTO regions VALUES(?1,?2)",
                params![id, self.regions[id].len],
            )?;
        }
        tx.commit()?;
        self.pending.clear();
        Ok(())
    }
    pub(crate) fn cached(&self, key: &str) -> Result<Option<Vec<PayloadId>>> {
        use rusqlite::OptionalExtension;
        let found = self
            .catalog
            .query_row("SELECT 1 FROM items WHERE key=?1", [key], |_| Ok(()))
            .optional()?
            .is_some();
        if !found {
            return Ok(None);
        }
        let mut stmt = self
            .catalog
            .prepare("SELECT id FROM members WHERE key=?1 ORDER BY nth")?;
        Ok(Some(
            stmt.query_map([key], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?,
        ))
    }
}
