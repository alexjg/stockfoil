use crate::{BLOCK_BYTES, BodyDecoder, Buffers, Error, FORMAT, Result, Spool, reply, sync_dir};
use ed25519_dalek::{Signer as _, SigningKey};
use rusqlite::params;
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
};
use stockfoil::{
    Author, CommitHash, ExportCursor, ExportedItem, FormatId, ItemId, Ledger, Next, PayloadId,
    Received, Reply, Request, Signature, WritePermissions,
};

struct Staged {
    ids: Vec<PayloadId>,
    references: usize,
}

/// One exclusively owned durable replica. The archive is transport-neutral;
/// all wire bytes and canonical members are streamed, including on restart.
/// Failed mutation/persistence poisons this handle: drop and reopen it.
pub struct Volume {
    dir: PathBuf,
    ledger: Ledger,
    spool: Spool,
    key: SigningKey,
    blocks: u64,
    staged: HashMap<ItemId, Staged>,
    saved: ExportCursor,
    poisoned: bool,
}
impl Volume {
    pub fn open(
        dir: impl AsRef<Path>,
        key: SigningKey,
        blocks: u64,
        buffers: &mut Buffers,
    ) -> Result<Self> {
        Self::open_with_capacity(dir, key, blocks, 1024, buffers)
    }
    pub fn open_with_capacity(
        dir: impl AsRef<Path>,
        key: SigningKey,
        blocks: u64,
        cap: usize,
        buffers: &mut Buffers,
    ) -> Result<Self> {
        buffers.validate()?;
        if blocks == 0 || blocks > i64::MAX as u64 / BLOCK_BYTES as u64 {
            return Err(Error::Invalid("invalid volume size"));
        }
        let dir = dir.as_ref().to_owned();
        std::fs::create_dir_all(dir.join("items"))?;
        let spool = Spool::open(&dir)?;
        spool.catalog.execute_batch("CREATE TABLE IF NOT EXISTS volume(id INTEGER PRIMARY KEY CHECK(id=0), blocks INTEGER NOT NULL)")?;
        spool
            .catalog
            .execute("INSERT OR IGNORE INTO volume VALUES(0,?1)", [blocks])?;
        let stored: u64 = spool
            .catalog
            .query_row("SELECT blocks FROM volume", [], |r| r.get(0))?;
        if stored != blocks {
            return Err(Error::Invalid("volume size changed"));
        }
        let author = Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap();
        let ledger = Ledger::new(FormatId::new(FORMAT)?, author).with_max_parked(cap);
        let mut this = Self {
            dir,
            ledger,
            spool,
            key,
            blocks,
            staged: HashMap::new(),
            saved: ExportCursor::new(),
            poisoned: false,
        };
        let entries: Vec<(String, Vec<u8>)> = {
            let mut stmt = this
                .spool
                .catalog
                .prepare("SELECT key,head FROM items ORDER BY key")?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?
        };
        // A capped queue can evict an item whose dependency appears later in
        // the archive. Retry unadmitted heads until a pass makes no progress.
        loop {
            let epoch = this.ledger.epoch();
            for (name, head) in &entries {
                let hash = CommitHash::from(
                    <[u8; 32]>::try_from(head.as_slice())
                        .map_err(|_| Error::Invalid("archive head"))?,
                );
                if !this.ledger.is_admitted(&hash) {
                    let file = File::open(this.dir.join("items").join(name))?;
                    let ids = this
                        .spool
                        .cached(name)?
                        .ok_or(Error::Invalid("missing archive cache"))?;
                    let op = this.verify(&file, Some(&ids), buffers)?;
                    this.place(op, ids)?;
                }
            }
            if this.ledger.epoch() == epoch {
                break;
            }
        }
        this.saved = this.full_cursor()?;
        sync_dir(&this.dir)?;
        if let Some(parent) = this.dir.parent() {
            sync_dir(parent)?;
        }
        Ok(this)
    }
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }
    pub fn spool(&self) -> &Spool {
        &self.spool
    }
    pub fn blocks(&self) -> u64 {
        self.blocks
    }
    pub fn staged_items(&self) -> usize {
        self.staged.len()
    }
    pub(crate) fn healthy(&self) -> Result<()> {
        if self.poisoned {
            Err(Error::Poisoned)
        } else {
            Ok(())
        }
    }
    pub fn set_permissions(&mut self, permissions: WritePermissions) -> Result<()> {
        self.healthy()?;
        self.ledger.set_permissions(permissions);
        Ok(())
    }
    fn full_cursor(&self) -> Result<ExportCursor> {
        let mut plan = self.ledger.export(&ExportCursor::new());
        for i in 0..plan.len() {
            plan.acknowledge(i)?;
        }
        Ok(plan.cursor())
    }
    /// Feed increasing indices, then exactly count complete blocks, without
    /// building a canonical payload in memory. Extra input is left unread.
    pub fn write(
        &mut self,
        indices: &[u64],
        input: &mut impl Read,
        parents: Option<Vec<CommitHash>>,
        buffers: &mut Buffers,
    ) -> Result<CommitHash> {
        self.healthy()?;
        buffers.validate()?;
        if indices.windows(2).any(|w| w[0] >= w[1])
            || indices.last().is_some_and(|b| *b >= self.blocks)
        {
            return Err(Error::Invalid("invalid local block indices"));
        }
        let mut builder = self.ledger.begin_commit(parents)?;
        let mut staging = self.spool.begin();
        let result = (|| {
            let mut put = |bytes: &[u8]| -> Result<()> {
                builder.update(bytes);
                self.spool.append(&mut staging, bytes)
            };
            put(&(indices.len() as u64).to_le_bytes())?;
            for b in indices {
                put(&b.to_le_bytes())?;
            }
            let mut left = indices.len() as u64 * BLOCK_BYTES as u64;
            while left > 0 {
                let n = left.min(buffers.read.len() as u64) as usize;
                input.read_exact(&mut buffers.read[..n])?;
                put(&buffers.read[..n])?;
                left -= n as u64;
            }
            let id = self.spool.finish(staging)?;
            self.publish(builder.finish(), id, buffers)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    /// Used to demonstrate application rejection of authenticated malformed data.
    pub fn commit_raw(
        &mut self,
        input: &mut impl Read,
        parents: Option<Vec<CommitHash>>,
        buffers: &mut Buffers,
    ) -> Result<CommitHash> {
        self.healthy()?;
        buffers.validate()?;
        let mut builder = self.ledger.begin_commit(parents)?;
        let mut staging = self.spool.begin();
        let result = (|| {
            loop {
                let n = input.read(&mut buffers.read)?;
                if n == 0 {
                    break;
                }
                builder.update(&buffers.read[..n]);
                self.spool.append(&mut staging, &buffers.read[..n])?;
            }
            let id = self.spool.finish(staging)?;
            self.publish(builder.finish(), id, buffers)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn publish(
        &mut self,
        prepared: stockfoil::Prepared,
        id: PayloadId,
        buffers: &mut Buffers,
    ) -> Result<CommitHash> {
        self.spool.sync()?;
        let draft = self.ledger.commit(prepared, id)?;
        let signature = Signature::from(self.key.sign(draft.signing_bytes()).to_bytes());
        let staged = &self.staged;
        let signed = self.ledger.attach_signature(
            &draft,
            signature,
            &mut |id: &ItemId, _: &[CommitHash]| staged[id].ids.clone(),
        )?;
        for receipt in signed.released {
            self.release(&receipt.id);
        }
        for (id, _) in signed.failed {
            self.release(&id);
            self.forget(&id)?;
        }
        let mut plan = self.ledger.export(&self.saved);
        for i in 0..plan.len() {
            let description = self.ledger.export_item(&plan, i)?;
            let temporary = self.dir.join("outgoing.tmp");
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(&temporary)?;
            self.export_item(&description, &mut file, buffers)?;
            let key = digest(&mut file, &mut buffers.read)?;
            self.archive(
                &temporary,
                &key,
                &description.id,
                &description
                    .members
                    .iter()
                    .map(|(_, id)| *id)
                    .collect::<Vec<_>>(),
            )?;
            plan.acknowledge(i)?;
        }
        self.saved = plan.cursor();
        Ok(draft.hash)
    }
    pub fn export_item(
        &self,
        item: &ExportedItem,
        output: &mut impl Write,
        buffers: &mut Buffers,
    ) -> Result<()> {
        self.healthy()?;
        buffers.validate()?;
        output.write_all(&item.header)?;
        for (_, id) in &item.members {
            let region = self.spool.region(*id)?;
            output.write_all(&region.len.to_le_bytes())?;
            let mut offset = 0;
            while offset < region.len {
                let n = (region.len - offset).min(buffers.read.len() as u64) as usize;
                self.spool.read(region, offset, &mut buffers.read[..n])?;
                output.write_all(&buffers.read[..n])?;
                offset += n as u64;
            }
        }
        Ok(())
    }
    pub fn receive(&mut self, input: &mut impl Read, buffers: &mut Buffers) -> Result<Received> {
        self.healthy()?;
        buffers.validate()?;
        let temporary = self.dir.join("incoming.tmp");
        let result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(&temporary)?;
            let mut hasher = blake3::Hasher::new();
            loop {
                let n = input.read(&mut buffers.read)?;
                if n == 0 {
                    break;
                }
                file.write_all(&buffers.read[..n])?;
                hasher.update(&buffers.read[..n]);
            }
            let key = hasher.finalize().to_hex().to_string();
            let cached = self.spool.cached(&key)?;
            let (op, ids) = self.verify_with_ids(&file, cached.as_deref(), buffers)?;
            let id = op.meta().unwrap().id.clone();
            let outcome = self.place(op, ids.clone())?;
            self.archive(&temporary, &key, &id, &ids)?;
            self.saved = self.full_cursor()?;
            Ok(outcome)
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }
    fn verify(
        &mut self,
        file: &File,
        ids: Option<&[PayloadId]>,
        buffers: &mut Buffers,
    ) -> Result<stockfoil::Admission> {
        Ok(self.verify_with_ids(file, ids, buffers)?.0)
    }
    fn verify_with_ids(
        &mut self,
        file: &File,
        cached: Option<&[PayloadId]>,
        buffers: &mut Buffers,
    ) -> Result<(stockfoil::Admission, Vec<PayloadId>)> {
        let mut options = self.ledger.receive_options();
        options.read_size = buffers.read.len();
        options.decode_capacity = buffers.decode.len();
        let mut op = self.ledger.begin_receive_with(options)?;
        let mut decoder = None;
        let mut staging = None;
        let mut ids = Vec::new();
        loop {
            match op.next() {
                Next::Continue => continue,
                Next::NeedHistory => break,
                Next::Failed(e) => return Err(e.clone().into()),
                Next::Request(ticket, Request::Read { offset, max }) => {
                    let n = file.read_at(&mut buffers.read[..max], offset)?;
                    op.reply(
                        ticket,
                        Reply::Read {
                            bytes: &buffers.read[..n],
                            eof: n == 0,
                        },
                    )
                    .map_err(reply)?;
                }
                Next::Request(
                    ticket,
                    Request::Decode {
                        meta,
                        input,
                        eof,
                        capacity,
                    },
                ) => {
                    let decoder = decoder.get_or_insert_with(|| BodyDecoder::new(meta.len()));
                    let (consumed, produced, status) =
                        decoder.step(input, eof, &mut buffers.decode[..capacity])?;
                    if cached.is_none() && status != stockfoil::DecodeStatus::Finished {
                        let current = staging.get_or_insert_with(|| self.spool.begin());
                        self.spool.append(current, &buffers.decode[..produced])?;
                        if status == stockfoil::DecodeStatus::EndMember {
                            ids.push(self.spool.finish(staging.take().unwrap())?);
                        }
                    }
                    op.reply(
                        ticket,
                        Reply::Decoded {
                            consumed,
                            output: &buffers.decode[..produced],
                            status,
                        },
                    )
                    .map_err(reply)?;
                }
                _ => return Err(Error::Invalid("unexpected verification state")),
            }
        }
        if let Some(cached) = cached {
            ids = cached.to_vec();
        }
        if ids.len() != op.members().unwrap().len() {
            return Err(Error::Invalid("cached member count"));
        }
        Ok((op, ids))
    }
    fn place(&mut self, op: stockfoil::Admission, ids: Vec<PayloadId>) -> Result<Received> {
        let id = op.meta().unwrap().id.clone();
        self.staged
            .entry(id.clone())
            .and_modify(|s| s.references += 1)
            .or_insert(Staged { ids, references: 1 });
        let staged = &self.staged;
        let outcome = self.ledger.admit(op, &mut |id: &ItemId, _: &[CommitHash]| {
            staged[id].ids.clone()
        });
        match &outcome {
            Ok(Received::Admitted { items, failed, .. }) => {
                for receipt in items {
                    self.release(&receipt.id);
                }
                for (id, _) in failed {
                    self.release(id);
                    self.forget(id)?;
                }
            }
            Ok(Received::Evicted { evicted, .. }) => self.release(evicted),
            Ok(Received::Waiting(_)) => {}
            Err(_) => self.release(&id),
        }
        Ok(outcome?)
    }
    fn release(&mut self, id: &ItemId) {
        if let Some(staged) = self.staged.get_mut(id) {
            staged.references -= 1;
            if staged.references == 0 {
                self.staged.remove(id);
            }
        }
    }
    fn forget(&self, id: &ItemId) -> Result<()> {
        let bytes = ExportCursor::from_items([id.clone()]).to_bytes();
        self.spool
            .catalog
            .execute("DELETE FROM items WHERE item_id=?1", [bytes])?;
        Ok(())
    }
    fn archive(
        &mut self,
        temporary: &Path,
        key: &str,
        id: &ItemId,
        ids: &[PayloadId],
    ) -> Result<()> {
        self.spool.sync()?;
        File::open(temporary)?.sync_all()?;
        std::fs::rename(temporary, self.dir.join("items").join(key))?;
        sync_dir(&self.dir.join("items"))?;
        let tx = self.spool.catalog.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO items VALUES(?1,?2,?3)",
            params![
                key,
                id.head().as_slice(),
                ExportCursor::from_items([id.clone()]).to_bytes()
            ],
        )?;
        for (nth, id) in ids.iter().enumerate() {
            tx.execute(
                "INSERT OR IGNORE INTO members VALUES(?1,?2,?3)",
                params![key, nth as u64, id],
            )?;
        }
        tx.commit()?;
        sync_dir(&self.dir)?;
        Ok(())
    }
}
fn digest(file: &mut File, buffer: &mut [u8]) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut offset = 0;
    loop {
        let n = file.read_at(buffer, offset)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        offset += n as u64;
    }
    Ok(hasher.finalize().to_hex().to_string())
}
