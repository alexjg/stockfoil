use crate::{
    Author, Bytes, Codec, CommitHash, DecodeStatus, Draft, Error, ExportCursor, ExportPlan,
    ExportedItem, Heads, History, ItemId, Ledger, Limits, Next, PayloadId, Received, Reply,
    Request, Signature, Signed, Signer, WritePermissions,
};
use std::collections::HashMap;

struct Staged {
    members: Vec<Bytes>,
    references: usize,
}

/// A ledger with canonical payloads in memory and a pure bundle codec.
///
/// Nothing does I/O or awaits: this is the small-document path. For storage
/// elsewhere use [`Ledger`] directly, assigning payload IDs as bytes stream
/// through caller-controlled buffers. Graph queries and advanced operations
/// remain available through [`ledger`](Self::ledger).
pub struct Document<C: Codec> {
    ledger: Ledger,
    codec: C,
    payloads: Vec<Bytes>,
    index: HashMap<CommitHash, PayloadId>,
    staged: HashMap<ItemId, Staged>,
}
impl<C: Codec> Document<C> {
    pub fn new(codec: C, author: Author) -> Self {
        Self {
            ledger: Ledger::new(codec.format().clone(), author),
            codec,
            payloads: Vec::new(),
            index: HashMap::new(),
            staged: HashMap::new(),
        }
    }
    pub fn with_permissions(mut self, permissions: WritePermissions) -> Self {
        self.ledger.set_permissions(permissions);
        self
    }
    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.ledger = self.ledger.with_limits(limits);
        self
    }
    pub fn with_max_parked(mut self, max: usize) -> Self {
        self.ledger = self.ledger.with_max_parked(max);
        self
    }
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }
    /// Primarily for policy and queries. Raw commits/admissions bypass the
    /// payload table: use document methods for operations involving bytes.
    pub fn ledger_mut(&mut self) -> &mut Ledger {
        &mut self.ledger
    }
    pub fn codec(&self) -> &C {
        &self.codec
    }
    pub fn author(&self) -> Author {
        self.ledger.author()
    }
    pub fn history(&self) -> &History {
        self.ledger.history()
    }
    pub fn heads(&self, which: Heads) -> Vec<CommitHash> {
        self.ledger.heads(which)
    }
    pub fn permissions(&self) -> &WritePermissions {
        self.ledger.permissions()
    }
    pub fn set_permissions(&mut self, permissions: WritePermissions) {
        self.ledger.set_permissions(permissions);
    }
    pub fn drafts(&self) -> impl Iterator<Item = Draft> + '_ {
        self.ledger.drafts()
    }
    pub fn has_drafts(&self) -> bool {
        self.ledger.has_drafts()
    }
    pub fn commit(
        &mut self,
        canonical: impl Into<Bytes>,
        parents: Option<Vec<CommitHash>>,
    ) -> Result<Draft, Error> {
        let bytes = canonical.into();
        let mut builder = self.ledger.begin_commit(parents)?;
        builder.update(&bytes);
        let prepared = builder.finish();
        let id = *self.index.entry(prepared.hash).or_insert_with(|| {
            let id = self.payloads.len() as PayloadId;
            self.payloads.push(bytes);
            id
        });
        self.ledger.commit(prepared, id)
    }
    pub fn payload(&self, hash: &CommitHash) -> Result<Bytes, Error> {
        let id = self
            .ledger
            .payload_id(hash)
            .ok_or(Error::UnknownCommit(*hash))?;
        self.payload_by_id(id)
    }
    pub(crate) fn payload_by_id(&self, id: PayloadId) -> Result<Bytes, Error> {
        self.payloads
            .get(id as usize)
            .cloned()
            .ok_or_else(|| Error::Storage("unknown in-memory payload ID".into()))
    }
    pub fn sign_drafts(&mut self, signer: &impl Signer) -> Result<Signed, Error> {
        let (payloads, index, staged) = (&mut self.payloads, &mut self.index, &self.staged);
        let signed = self
            .ledger
            .sign_drafts(signer, &mut |id: &ItemId, hashes: &[CommitHash]| {
                bind(staged, payloads, index, id, hashes)
            })?;
        self.clean_signed(&signed);
        Ok(signed)
    }
    pub fn attach_signature(
        &mut self,
        draft: &Draft,
        signature: Signature,
    ) -> Result<Signed, Error> {
        let (payloads, index, staged) = (&mut self.payloads, &mut self.index, &self.staged);
        let signed = self.ledger.attach_signature(
            draft,
            signature,
            &mut |id: &ItemId, hashes: &[CommitHash]| bind(staged, payloads, index, id, hashes),
        )?;
        self.clean_signed(&signed);
        Ok(signed)
    }
    /// Verify, admit or park one item. Staging is shared for duplicate parked
    /// copies; admission and eviction release their staging references.
    pub fn receive(&mut self, item: &[u8]) -> Result<Received, Error> {
        let mut op = self.ledger.begin_memory_receive(item.len());
        let mut members: Option<Vec<Bytes>> = None;
        let (mut member, mut offset) = (0usize, 0usize);
        loop {
            match op.next() {
                Next::Continue => continue,
                Next::NeedHistory => break,
                Next::Failed(e) => return Err(e.clone()),
                Next::Request(..) | Next::AwaitReply => {}
                _ => return Err(Error::Protocol("unexpected in-memory verification state")),
            }
            let (ticket, request) = op
                .request()
                .ok_or(Error::Protocol("missing admission request"))?;
            match request {
                Request::Read { offset, max } => {
                    let start = usize::try_from(offset)
                        .unwrap_or(usize::MAX)
                        .min(item.len());
                    let end = start.saturating_add(max).min(item.len());
                    op.reply(
                        ticket,
                        Reply::Read {
                            bytes: &item[start..end],
                            eof: end == item.len(),
                        },
                    )
                    .map_err(crate::ledger::reply_error)?;
                }
                Request::Decode {
                    meta,
                    input,
                    capacity,
                    ..
                } => {
                    let input_len = input.len();
                    let first = members.is_none();
                    if first {
                        let decoded = self
                            .codec
                            .decode(meta, input)
                            .map_err(|e| Error::Codec(e.to_string()))?;
                        if decoded.len() != meta.len() {
                            return Err(Error::Codec("wrong number of decoded members".into()));
                        }
                        members = Some(decoded);
                    }
                    let decoded = members.as_ref().unwrap();
                    let consumed = if first { input_len } else { 0 };
                    let (output, status) = if member < decoded.len() {
                        let end = offset.saturating_add(capacity).min(decoded[member].len());
                        let output = &decoded[member][offset..end];
                        let status = if end == decoded[member].len() {
                            DecodeStatus::EndMember
                        } else {
                            DecodeStatus::Working
                        };
                        if status == DecodeStatus::EndMember {
                            member += 1;
                            offset = 0;
                        } else {
                            offset = end;
                        }
                        (output, status)
                    } else {
                        (&[][..], DecodeStatus::Finished)
                    };
                    op.reply(
                        ticket,
                        Reply::Decoded {
                            consumed,
                            output,
                            status,
                        },
                    )
                    .map_err(crate::ledger::reply_error)?;
                }
                Request::Bind { .. } => {
                    return Err(Error::Protocol("verification attempted to bind"));
                }
            }
        }
        let id = op.meta().unwrap().id.clone();
        self.staged
            .entry(id.clone())
            .and_modify(|s| s.references += 1)
            .or_insert(Staged {
                members: members.unwrap_or_default(),
                references: 1,
            });
        let (payloads, index, staged) = (&mut self.payloads, &mut self.index, &self.staged);
        let outcome = self
            .ledger
            .admit(op, &mut |id: &ItemId, hashes: &[CommitHash]| {
                bind(staged, payloads, index, id, hashes)
            });
        match &outcome {
            Ok(Received::Admitted { items, failed, .. }) => {
                for receipt in items {
                    self.release(&receipt.id);
                }
                for (id, _) in failed {
                    self.release(id);
                }
            }
            Ok(Received::Evicted { evicted, .. }) => self.release(evicted),
            Ok(Received::Waiting(_)) => {}
            Err(_) => self.release(&id),
        }
        outcome
    }
    fn release(&mut self, id: &ItemId) {
        if let Some(staged) = self.staged.get_mut(id) {
            staged.references -= 1;
            if staged.references == 0 {
                self.staged.remove(id);
            }
        }
    }
    fn clean_signed(&mut self, signed: &Signed) {
        for receipt in &signed.released {
            self.release(&receipt.id);
        }
        for (id, _) in &signed.failed {
            self.release(id);
        }
    }
    /// Encode from an owned plan with only a short borrow of the document.
    /// The caller can then await storage/network I/O and keep editing before
    /// acknowledging the plan; no document borrow has to cross that await.
    pub fn export_item(
        &self,
        plan: &ExportPlan,
        index: usize,
    ) -> Result<(ExportedItem, Vec<u8>), Error> {
        let item = self.ledger.export_item(plan, index)?;
        let members = item
            .members
            .iter()
            .map(|(_, id)| self.payload_by_id(*id))
            .collect::<Result<Vec<_>, _>>()?;
        let mut bytes = item.header.clone();
        bytes.extend(
            self.codec
                .encode(&item.meta, &members)
                .map_err(|e| Error::Codec(e.to_string()))?,
        );
        Ok((item, bytes))
    }

    pub fn export(&self, cursor: &ExportCursor) -> Export<'_, C> {
        Export {
            document: self,
            plan: self.ledger.export(cursor),
        }
    }
}
fn bind(
    staged: &HashMap<ItemId, Staged>,
    payloads: &mut Vec<Bytes>,
    index: &mut HashMap<CommitHash, PayloadId>,
    item: &ItemId,
    hashes: &[CommitHash],
) -> Vec<PayloadId> {
    let members = &staged.get(item).expect("verified member staging").members;
    hashes
        .iter()
        .zip(members)
        .map(|(hash, bytes)| {
            *index.entry(*hash).or_insert_with(|| {
                let id = payloads.len() as PayloadId;
                payloads.push(bytes.clone());
                id
            })
        })
        .collect()
}

/// A captured in-memory export, acknowledged only after the destination has it.
pub struct Export<'a, C: Codec> {
    document: &'a Document<C>,
    plan: ExportPlan,
}
impl<C: Codec> Export<'_, C> {
    pub fn len(&self) -> usize {
        self.plan.len()
    }
    pub fn is_empty(&self) -> bool {
        self.plan.is_empty()
    }
    pub fn item(&self, index: usize) -> Result<(ExportedItem, Vec<u8>), Error> {
        self.document.export_item(&self.plan, index)
    }
    pub fn acknowledge(&mut self, index: usize) -> Result<(), Error> {
        self.plan.acknowledge(index)
    }
    pub fn cursor(&self) -> ExportCursor {
        self.plan.cursor()
    }
    pub fn plan(&self) -> &ExportPlan {
        &self.plan
    }
    /// Release the document borrow while keeping a captured, acknowledgeable plan.
    pub fn into_plan(self) -> ExportPlan {
        self.plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::Evidence;
    use crate::{Concatenated, FormatId, FragmentRange, MemberShape, ParentRef};
    use ed25519_dalek::SigningKey;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    #[test]
    fn failed_parked_placement_does_not_hide_successful_receipts_or_leak_staging() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let format = FormatId::new("test/concatenated-v1").unwrap();
        let mut sender = Document::new(Concatenated::new(format.clone()), Author::from(&key));
        let base = sender.commit(&b"base"[..], None).unwrap();
        sender.sign_drafts(&key).unwrap();
        let base_item = sender.export(&ExportCursor::new()).item(0).unwrap().1;
        // Byte-authenticated but incorrectly placed fragment: it omits its
        // external parent's history without declaring a boundary.
        let (hash, payload) = (0u64..)
            .find_map(|nonce| {
                let bytes = nonce.to_le_bytes().to_vec();
                let mut builder = sender
                    .history()
                    .begin_commit(Author::from(&key), vec![base.hash])
                    .unwrap();
                builder.update(&bytes);
                let hash = builder.finish().hash;
                (crate::meta::depth(&hash) == 1).then_some((hash, Bytes::from(bytes)))
            })
            .unwrap();
        let id = ItemId::Fragment(FragmentRange {
            head: hash,
            boundary: BTreeSet::new(),
        });
        let meta = crate::BundleMeta {
            id: id.clone(),
            members: vec![MemberShape {
                author: Author::from(&key),
                parents: vec![ParentRef::External(base.hash)],
            }],
            evidence: vec![Evidence {
                author: Author::from(&key),
                member: 0,
                signature: Signature::from(ed25519_dalek::Signer::sign(&key, hash.as_bytes())),
            }],
        };
        let mut bad = crate::wire::encode_header(&meta, &format);
        bad.extend(sender.codec().encode(&meta, &[payload]).unwrap());
        let mut receiver = Document::new(Concatenated::new(format), Author::from(&key));
        assert_eq!(
            receiver.receive(&bad).unwrap(),
            Received::Waiting(base.hash)
        );
        assert_eq!(receiver.staged.len(), 1);
        let Received::Admitted {
            items,
            delivered,
            failed,
        } = receiver.receive(&base_item).unwrap()
        else {
            panic!()
        };
        assert_eq!(items.len(), 1);
        assert_eq!(delivered, [base.hash]);
        assert_eq!(failed, [(id, Error::InvalidPlacement)]);
        assert!(receiver.staged.is_empty());
        assert_eq!(receiver.payloads.len(), 1);
        assert!(!receiver.ledger().contains(&hash));
    }

    #[test]
    fn an_evictable_duplicate_uses_one_staging_entry_and_one_stored_payload() {
        let key = SigningKey::from_bytes(&[1; 32]);
        let format = FormatId::new("test/duplicate-v1").unwrap();
        let mut sender = Document::new(Concatenated::new(format.clone()), Author::from(&key));
        let mut items = Vec::new();
        let mut cursor = ExportCursor::new();
        for payload in [b"base".as_slice(), b"child".as_slice()] {
            sender.commit(payload, None).unwrap();
            sender.sign_drafts(&key).unwrap();
            let mut export = sender.export(&cursor);
            assert_eq!(export.len(), 1);
            items.push(export.item(0).unwrap().1);
            export.acknowledge(0).unwrap();
            cursor = export.cursor();
        }
        let mut receiver =
            Document::new(Concatenated::new(format), Author::from(&key)).with_max_parked(1);
        for _ in 0..10 {
            receiver.receive(&items[1]).unwrap();
        }
        assert_eq!(receiver.staged.len(), 1);
        assert_eq!(receiver.staged.values().next().unwrap().references, 1);
        assert!(receiver.payloads.is_empty());
        receiver.receive(&items[0]).unwrap();
        assert!(receiver.staged.is_empty());
        assert_eq!(receiver.payloads.len(), 2);
        let weak: Vec<_> = receiver.payloads.iter().map(Arc::downgrade).collect();
        drop(receiver);
        assert!(weak.iter().all(|w| w.upgrade().is_none()));
    }
}
