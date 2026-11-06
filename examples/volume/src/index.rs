use crate::{BLOCK_BYTES, Error, Result, Volume, db, format::BlockWrite};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs::{File, OpenOptions},
    path::Path,
};
use stockfoil::{Ancestry, Applied, Batch, Checkpoint, CommitHash, Event, Follow, Materialization};

/// On-disk delivered writes, participation and per-block winners. No block-sized
/// or volume-sized index is loaded into RAM. Checkpoint and index share a SQL tx.
pub struct BlockIndex {
    pub(crate) conn: Connection,
    pub(crate) planner: Materialization,
    blocks: u64,
    pub(crate) poisoned: bool,
    _lock: File,
}
impl BlockIndex {
    pub fn open(path: &Path, blocks: u64, follow: Follow) -> Result<Self> {
        if blocks == 0 || blocks > i64::MAX as u64 / BLOCK_BYTES as u64 {
            return Err(Error::Invalid("invalid index size"));
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        lock.try_lock()
            .map_err(|_| Error::Invalid("index already open"))?;
        let conn = db(path)?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS state(id INTEGER PRIMARY KEY CHECK(id=0), blocks INTEGER NOT NULL, generation INTEGER NOT NULL, checkpoint BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS commits(hash BLOB PRIMARY KEY, count INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS writes(block INTEGER NOT NULL, hash BLOB NOT NULL, nth INTEGER NOT NULL, PRIMARY KEY(block,hash));
            CREATE INDEX IF NOT EXISTS by_hash ON writes(hash);
            CREATE TABLE IF NOT EXISTS active(hash BLOB PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS current(block INTEGER PRIMARY KEY, hash BLOB NOT NULL, nth INTEGER NOT NULL);
            CREATE TEMP TABLE affected(block INTEGER PRIMARY KEY);
            CREATE TEMP TABLE pending(block INTEGER PRIMARY KEY, hash BLOB NOT NULL, nth INTEGER NOT NULL);")?;
        conn.execute(
            "INSERT OR IGNORE INTO state VALUES(0,?1,0,?2)",
            params![blocks, Checkpoint::new(follow).to_bytes()],
        )?;
        let (stored, cp): (u64, Vec<u8>) =
            conn.query_row("SELECT blocks,checkpoint FROM state", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        if stored != blocks {
            return Err(Error::Invalid("index volume size changed"));
        }
        let planner = Materialization::resume(Checkpoint::from_bytes(&cp)?, true);
        Ok(Self {
            conn,
            planner,
            blocks,
            poisoned: false,
            _lock: lock,
        })
    }
    pub fn checkpoint(&self) -> &Checkpoint {
        self.planner.checkpoint()
    }
    pub fn set_follow(&mut self, follow: Follow) {
        self.planner.set_follow(follow);
    }
    pub fn is_current(&self, volume: &Volume) -> bool {
        !self.poisoned && self.planner.is_current(volume.ledger())
    }
    pub fn step(&mut self, volume: &Volume, budget: usize) -> Result<bool> {
        volume.healthy()?;
        if budget == 0 || volume.blocks() != self.blocks {
            return Err(Error::Invalid("invalid index budget/size"));
        }
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        if self.is_current(volume) {
            return Ok(true);
        }
        let saved = self.planner.checkpoint().clone();
        let result = (|| {
            let tx = self.conn.unchecked_transaction()?;
            let mut next = self.planner.plan(volume.ledger(), budget)?;
            while let Some(batch) = next {
                let applied = apply(&tx, volume, &batch)?;
                changes(&tx, volume, |_, _| Ok(()))?;
                next = self.planner.complete(volume.ledger(), batch, applied)?;
            }
            save(&tx, self.planner.checkpoint(), generation(&tx)?)?;
            tx.commit()?;
            Ok(self.planner.is_current(volume.ledger()))
        })();
        if result.is_err() {
            self.planner = Materialization::resume(saved, true);
            self.poisoned = true;
        }
        result
    }
    pub fn update(&mut self, volume: &Volume, budget: usize) -> Result<()> {
        while !self.step(volume, budget)? {}
        Ok(())
    }
    pub fn winner_at(
        &self,
        volume: &Volume,
        at: &Ancestry,
        block: u64,
    ) -> Result<Option<CommitHash>> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        winner(&self.conn, volume, block, Some(at))
    }
    /// A lazy read uses this index plus an owned ancestry, not an image. Zeroes
    /// are always written for holes, even when the caller reuses a dirty buffer.
    pub fn read_at(
        &self,
        volume: &Volume,
        heads: &[CommitHash],
        block: u64,
        out: &mut [u8; BLOCK_BYTES],
    ) -> Result<()> {
        volume.healthy()?;
        if block >= self.blocks {
            return Err(Error::Invalid("block beyond volume"));
        }
        if !self.is_current(volume) {
            return Err(Error::Invalid("lazy index is stale"));
        }
        let at = volume.ledger().ancestry(heads)?;
        match self.winner_at(volume, &at, block)? {
            Some(hash) => {
                let nth: u64 = self.conn.query_row(
                    "SELECT nth FROM writes WHERE block=?1 AND hash=?2",
                    params![block, hash.as_slice()],
                    |r| r.get(0),
                )?;
                read_write(volume, hash, nth, out)
            }
            None => {
                out.fill(0);
                Ok(())
            }
        }
    }
}
pub(crate) fn generation(conn: &Connection) -> Result<u64> {
    Ok(conn.query_row("SELECT generation FROM state", [], |r| r.get(0))?)
}
pub(crate) fn save(conn: &Connection, cp: &Checkpoint, generation: u64) -> Result<()> {
    conn.execute(
        "UPDATE state SET generation=?1,checkpoint=?2",
        params![generation, cp.to_bytes()],
    )?;
    Ok(())
}
fn hash(bytes: Vec<u8>) -> Result<CommitHash> {
    Ok(CommitHash::from(
        <[u8; 32]>::try_from(bytes).map_err(|_| Error::Invalid("index hash"))?,
    ))
}
pub(crate) fn read_write(
    volume: &Volume,
    hash: CommitHash,
    nth: u64,
    out: &mut [u8; BLOCK_BYTES],
) -> Result<()> {
    let id = volume
        .ledger()
        .payload_id(&hash)
        .ok_or(Error::Invalid("index references missing commit"))?;
    let region = volume.spool().region(id)?;
    // Header was validated at delivery; stores are trusted.
    let mut word = [0; 8];
    volume.spool().read(region, 0, &mut word)?;
    BlockWrite {
        region,
        count: u64::from_le_bytes(word),
    }
    .read(volume.spool(), nth, out)
}
fn winner(
    conn: &Connection,
    volume: &Volume,
    block: u64,
    at: Option<&Ancestry>,
) -> Result<Option<CommitHash>> {
    let sql = if at.is_some() {
        "SELECT hash FROM writes WHERE block=?1"
    } else {
        "SELECT writes.hash FROM writes JOIN active USING(hash) WHERE block=?1"
    };
    let mut stmt = conn.prepare(sql)?;
    let mut maximal = Vec::new(); // at most graph-sized, never block/byte-sized
    for row in stmt.query_map([block], |r| r.get::<_, Vec<u8>>(0))? {
        let h = hash(row?)?;
        if let Some(at) = at
            && !at.participates(volume.ledger().history(), &h)?
        {
            continue;
        }
        if maximal.iter().any(|d| volume.ledger().is_ancestor(&h, d)) {
            continue;
        }
        maximal.retain(|a| !volume.ledger().is_ancestor(a, &h));
        maximal.push(h);
    }
    Ok(maximal.into_iter().max())
}

pub(crate) fn apply(conn: &Connection, volume: &Volume, batch: &Batch) -> Result<Applied> {
    let mut applied = Applied::default();
    conn.execute("DELETE FROM affected", [])?;
    for event in batch.events() {
        match event {
            Event::Reset => {
                conn.execute_batch("INSERT OR IGNORE INTO affected SELECT block FROM current;
                    DELETE FROM current; DELETE FROM active; DELETE FROM writes; DELETE FROM commits; DELETE FROM pending;")?;
            }
            Event::Deliver {
                hash,
                payload,
                active,
                ..
            } => {
                let region = volume.spool().region(*payload)?;
                let write = match BlockWrite::parse(volume.spool(), region, volume.blocks()) {
                    Ok(write) => write,
                    Err(Error::Invalid(reason)) => {
                        applied = applied.reject(*hash, reason);
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                conn.execute(
                    "INSERT OR REPLACE INTO commits VALUES(?1,?2)",
                    params![hash.as_slice(), write.count],
                )?;
                let mut insert =
                    conn.prepare_cached("INSERT OR REPLACE INTO writes VALUES(?1,?2,?3)")?;
                for nth in 0..write.count {
                    let block = write.block(volume.spool(), nth)?;
                    insert.execute(params![block, hash.as_slice(), nth])?;
                }
                if *active {
                    conn.execute("INSERT OR IGNORE INTO active VALUES(?1)", [hash.as_slice()])?;
                    affect(conn, hash)?;
                }
            }
            Event::Activate { hash } => {
                conn.execute("INSERT OR IGNORE INTO active VALUES(?1)", [hash.as_slice()])?;
                affect(conn, hash)?;
            }
            Event::Retract { hash } => {
                conn.execute("DELETE FROM active WHERE hash=?1", [hash.as_slice()])?;
                affect(conn, hash)?;
            }
            Event::Payload { .. } => {}
        }
    }
    Ok(applied)
}
fn affect(conn: &Connection, hash: &CommitHash) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO affected SELECT block FROM writes WHERE hash=?1",
        [hash.as_slice()],
    )?;
    Ok(())
}
/// Recompute current winners, passing only changed blocks to the host. This
/// does not hold an entire dirty-block list in memory, even for a huge commit.
pub(crate) fn changes(
    conn: &Connection,
    volume: &Volume,
    mut changed: impl FnMut(u64, Option<(CommitHash, u64)>) -> Result<()>,
) -> Result<()> {
    let mut stmt = conn.prepare("SELECT block FROM affected ORDER BY block")?;
    for row in stmt.query_map([], |r| r.get::<_, u64>(0))? {
        let block = row?;
        let winner = winner(conn, volume, block, None)?;
        let old: Option<Vec<u8>> = conn
            .query_row("SELECT hash FROM current WHERE block=?1", [block], |r| {
                r.get(0)
            })
            .optional()?;
        if old.as_deref() == winner.as_ref().map(|h| h.as_slice()) {
            continue;
        }
        let new = match winner {
            Some(hash) => {
                let nth: u64 = conn.query_row(
                    "SELECT nth FROM writes WHERE block=?1 AND hash=?2",
                    params![block, hash.as_slice()],
                    |r| r.get(0),
                )?;
                conn.execute(
                    "INSERT OR REPLACE INTO current VALUES(?1,?2,?3)",
                    params![block, hash.as_slice(), nth],
                )?;
                Some((hash, nth))
            }
            None => {
                conn.execute("DELETE FROM current WHERE block=?1", [block])?;
                None
            }
        };
        changed(block, new)?;
    }
    Ok(())
}
