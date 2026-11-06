use crate::{BLOCK_BYTES, BlockIndex, Buffers, Error, Result, Volume, index, sync_dir};
use rusqlite::params;
use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
};
use stockfoil::{Batch, CommitHash, Event, Follow};

const MAGIC: &[u8; 5] = b"SFVJ\x01";

/// Deterministic crash points for testing the actual on-disk protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    None,
    BeforeJournalSync,
    AfterJournalSync,
    AfterIndexCommit,
    DuringImage,
    AfterImageSync,
}
fn interrupt(actual: Fault, here: Fault) -> Result<()> {
    if actual == here {
        Err(Error::Interrupted(here))
    } else {
        Ok(())
    }
}

/// Flat sparse file plus SQL index/checkpoint, coupled by a durable redo log.
/// Never read image.raw externally while a transaction/recovery is in flight.
pub struct DiskImage {
    dir: PathBuf,
    file: File,
    index: BlockIndex,
    blocks: u64,
}
impl DiskImage {
    pub fn open(
        dir: impl AsRef<Path>,
        blocks: u64,
        follow: Follow,
        buffers: &mut Buffers,
    ) -> Result<Self> {
        buffers.validate()?;
        if blocks == 0 || blocks > i64::MAX as u64 / BLOCK_BYTES as u64 {
            return Err(Error::Invalid("invalid image size"));
        }
        let dir = dir.as_ref().to_owned();
        std::fs::create_dir_all(&dir)?;
        let index = BlockIndex::open(&dir.join("index.sqlite"), blocks, follow)?;
        let initial = index::generation(&index.conn)? == 0;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(initial)
            .truncate(false)
            .open(dir.join("image.raw"))?;
        if initial {
            file.set_len(0)?;
            file.set_len(blocks * BLOCK_BYTES as u64)?;
            file.sync_all()?;
        }
        let this = Self {
            dir,
            file,
            index,
            blocks,
        };
        this.recover(buffers)?;
        if this.file.metadata()?.len() != blocks * BLOCK_BYTES as u64 {
            return Err(Error::Invalid("truncated image"));
        }
        sync_dir(&this.dir)?;
        if let Some(parent) = this.dir.parent() {
            sync_dir(parent)?;
        }
        Ok(this)
    }
    pub fn checkpoint(&self) -> &stockfoil::Checkpoint {
        self.index.checkpoint()
    }
    pub fn set_follow(&mut self, follow: Follow) {
        self.index.set_follow(follow);
    }
    pub fn is_current(&self, volume: &Volume) -> bool {
        self.index.is_current(volume)
    }
    pub fn read(&self, block: u64, out: &mut [u8; BLOCK_BYTES]) -> Result<()> {
        if self.index.poisoned {
            return Err(Error::Poisoned);
        }
        if block >= self.blocks {
            return Err(Error::Invalid("block beyond image"));
        }
        self.file.read_exact_at(out, block * BLOCK_BYTES as u64)?;
        Ok(())
    }
    pub fn step(&mut self, volume: &Volume, budget: usize, buffers: &mut Buffers) -> Result<bool> {
        self.step_with_fault(volume, budget, buffers, Fault::None)
    }
    pub fn update(&mut self, volume: &Volume, budget: usize, buffers: &mut Buffers) -> Result<()> {
        while !self.step(volume, budget, buffers)? {}
        Ok(())
    }
    pub fn step_with_fault(
        &mut self,
        volume: &Volume,
        budget: usize,
        buffers: &mut Buffers,
        fault: Fault,
    ) -> Result<bool> {
        volume.healthy()?;
        buffers.validate()?;
        if budget == 0 {
            return Err(Error::Invalid("zero planner budget"));
        }
        if self.index.poisoned {
            return Err(Error::Poisoned);
        }
        if volume.blocks() != self.blocks {
            return Err(Error::Invalid("image/volume size mismatch"));
        }
        if self.is_current(volume) {
            return Ok(true);
        }
        let saved = self.index.planner.checkpoint().clone();
        let result = (|| {
            let tx = self.index.conn.unchecked_transaction()?;
            let next_generation = index::generation(&tx)?
                .checked_add(1)
                .ok_or(Error::Invalid("generation exhausted"))?;
            let temporary = self.dir.join("journal.tmp");
            let mut journal =
                Journal::new(File::create(&temporary)?, next_generation, self.blocks)?;
            let mut batch = self.index.planner.plan(volume.ledger(), budget)?;
            while let Some(current) = batch {
                let mut applied = index::apply(&tx, volume, &current)?;
                if current.is_rebuild() {
                    journal.reset()?;
                }
                let available: BTreeSet<_> = current
                    .events()
                    .iter()
                    .filter_map(|e| match e {
                        Event::Deliver { hash, .. } | Event::Payload { hash, .. } => Some(*hash),
                        _ => None,
                    })
                    .collect();
                let mut wanted = BTreeSet::new();
                index::changes(&tx, volume, |block, winner| {
                    tx.execute("DELETE FROM pending WHERE block=?1", [block])?;
                    match winner {
                        None => journal.zero(block),
                        Some((hash, nth)) if available.contains(&hash) => {
                            journal.copy(volume, hash, nth, block, buffers)
                        }
                        Some((hash, nth)) => {
                            tx.execute(
                                "INSERT INTO pending VALUES(?1,?2,?3)",
                                params![block, hash.as_slice(), nth],
                            )?;
                            wanted.insert(hash);
                            Ok(())
                        }
                    }
                })?;
                // Older winners arrive through real planner payload rounds.
                copy_requested(&tx, volume, &current, &mut journal, buffers)?;
                applied.wanted.extend(wanted);
                batch = self
                    .index
                    .planner
                    .complete(volume.ledger(), current, applied)?;
            }
            interrupt(fault, Fault::BeforeJournalSync)?;
            journal.finish()?; // footer + checksum + fsync
            std::fs::rename(&temporary, self.dir.join("journal"))?;
            sync_dir(&self.dir)?;
            interrupt(fault, Fault::AfterJournalSync)?;
            index::save(&tx, self.index.planner.checkpoint(), next_generation)?;
            tx.commit()?; // EXTRA syncs rollback-journal deletion as well as DB pages
            sync_dir(&self.dir)?; // the commit must be durable before touching image bytes
            interrupt(fault, Fault::AfterIndexCommit)?;
            self.replay(buffers, fault)?;
            interrupt(fault, Fault::AfterImageSync)?;
            std::fs::remove_file(self.dir.join("journal"))?;
            sync_dir(&self.dir)?;
            Ok(self.is_current(volume))
        })();
        // SQL commit errors can be ambiguous; neither speculative planner state
        // nor a partially overwritten image may escape through this handle.
        if result.is_err() {
            self.index.planner = stockfoil::Materialization::resume(saved, true);
            self.index.poisoned = true;
        }
        result
    }
    fn recover(&self, buffers: &mut Buffers) -> Result<()> {
        let path = self.dir.join("journal");
        if !path.try_exists()? {
            return Ok(());
        }
        let (generation, blocks) = verify_journal(&path, &mut buffers.read)?;
        if blocks != self.blocks {
            return Err(Error::Invalid("journal volume mismatch"));
        }
        let committed = index::generation(&self.index.conn)?;
        if generation == committed {
            self.replay(buffers, Fault::None)?;
        } else if generation
            != committed
                .checked_add(1)
                .ok_or(Error::Invalid("generation exhausted"))?
        {
            return Err(Error::Invalid("journal generation mismatch"));
        }
        // A journal one generation ahead never touched the image: SQL rolled
        // back. An equal generation has now been completely redone.
        std::fs::remove_file(path)?;
        sync_dir(&self.dir)?;
        Ok(())
    }
    fn replay(&self, buffers: &mut Buffers, fault: Fault) -> Result<()> {
        let path = self.dir.join("journal");
        let (generation, blocks) = verify_journal(&path, &mut buffers.read)?;
        if generation != index::generation(&self.index.conn)? || blocks != self.blocks {
            return Err(Error::Invalid("cannot replay uncommitted journal"));
        }
        let mut reader = File::open(path)?;
        let mut header = [0; 21];
        reader.read_exact(&mut header)?;
        let mut first = true;
        loop {
            let mut tag = [0];
            reader.read_exact(&mut tag)?;
            if tag[0] == 255 {
                break;
            }
            match tag[0] {
                0 => {
                    self.file.set_len(0)?;
                    self.file.set_len(self.blocks * BLOCK_BYTES as u64)?;
                }
                1 | 2 => {
                    let mut word = [0; 8];
                    reader.read_exact(&mut word)?;
                    let block = u64::from_le_bytes(word);
                    if block >= self.blocks {
                        return Err(Error::Invalid("journal block beyond image"));
                    }
                    if tag[0] == 1 {
                        buffers.block.fill(0);
                    } else {
                        reader.read_exact(&mut buffers.block[..])?;
                    }
                    // Split the write to test a crash inside a block overwrite.
                    let offset = block * BLOCK_BYTES as u64;
                    let half = BLOCK_BYTES / 2;
                    self.file.write_all_at(&buffers.block[..half], offset)?;
                    if first {
                        interrupt(fault, Fault::DuringImage)?;
                    }
                    self.file
                        .write_all_at(&buffers.block[half..], offset + half as u64)?;
                }
                _ => return Err(Error::Invalid("journal opcode")),
            }
            first = false;
        }
        self.file.sync_all()?;
        Ok(())
    }
}
fn copy_requested(
    conn: &rusqlite::Connection,
    volume: &Volume,
    batch: &Batch,
    journal: &mut Journal,
    buffers: &mut Buffers,
) -> Result<()> {
    for event in batch.events() {
        if let Event::Payload { hash, .. } = event {
            let mut stmt =
                conn.prepare("SELECT block,nth FROM pending WHERE hash=?1 ORDER BY block")?;
            for row in stmt.query_map([hash.as_slice()], |r| {
                Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?))
            })? {
                let (block, nth) = row?;
                journal.copy(volume, *hash, nth, block, buffers)?;
            }
            conn.execute("DELETE FROM pending WHERE hash=?1", [hash.as_slice()])?;
        }
    }
    Ok(())
}
struct Journal {
    file: File,
    hasher: blake3::Hasher,
}
impl Journal {
    fn new(file: File, generation: u64, blocks: u64) -> Result<Self> {
        let mut this = Self {
            file,
            hasher: blake3::Hasher::new(),
        };
        this.put(MAGIC)?;
        this.put(&generation.to_le_bytes())?;
        this.put(&blocks.to_le_bytes())?;
        Ok(this)
    }
    fn put(&mut self, bytes: &[u8]) -> Result<()> {
        self.file.write_all(bytes)?;
        self.hasher.update(bytes);
        Ok(())
    }
    fn reset(&mut self) -> Result<()> {
        self.put(&[0])
    }
    fn zero(&mut self, block: u64) -> Result<()> {
        self.put(&[1])?;
        self.put(&block.to_le_bytes())
    }
    fn copy(
        &mut self,
        volume: &Volume,
        hash: CommitHash,
        nth: u64,
        block: u64,
        buffers: &mut Buffers,
    ) -> Result<()> {
        index::read_write(volume, hash, nth, &mut buffers.block)?;
        self.put(&[2])?;
        self.put(&block.to_le_bytes())?;
        self.put(&buffers.block[..])
    }
    fn finish(mut self) -> Result<()> {
        self.put(&[255])?;
        self.file.write_all(self.hasher.finalize().as_bytes())?;
        self.file.sync_all()?;
        Ok(())
    }
}
fn verify_journal(path: &Path, buffer: &mut [u8]) -> Result<(u64, u64)> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    if length < 54 {
        return Err(Error::Invalid("short journal"));
    }
    let mut header = [0; 21];
    file.read_exact(&mut header)?;
    if &header[..5] != MAGIC {
        return Err(Error::Invalid("journal version"));
    }
    let generation = u64::from_le_bytes(header[5..13].try_into().unwrap());
    let blocks = u64::from_le_bytes(header[13..21].try_into().unwrap());
    let mut hasher = blake3::Hasher::new();
    let mut offset = 0;
    while offset < length - 32 {
        let n = (length - 32 - offset).min(buffer.len() as u64) as usize;
        file.read_exact_at(&mut buffer[..n], offset)?;
        hasher.update(&buffer[..n]);
        offset += n as u64;
    }
    let mut expected = [0; 32];
    file.read_exact_at(&mut expected, length - 32)?;
    if hasher.finalize().as_bytes() != &expected {
        return Err(Error::Invalid("journal checksum"));
    }
    let mut end = [0];
    file.read_exact_at(&mut end, length - 33)?;
    if end != [255] {
        return Err(Error::Invalid("journal footer"));
    }
    Ok((generation, blocks))
}
