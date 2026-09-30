use ed25519_dalek::{Signer, SigningKey};
use std::{cell::Cell, collections::BTreeMap, ops::ControlFlow, rc::Rc};
use stockfoil::*;

#[derive(Clone, Default)]
struct Codec {
    changed: Rc<Cell<bool>>,
    fail: Rc<Cell<bool>>,
    bad_count: Rc<Cell<bool>>,
    permissive: bool,
    id: Option<&'static str>,
}
fn err() -> std::io::Error {
    std::io::Error::other("test codec failure")
}
impl PayloadCodec for Codec {
    type Payload = String;
    type Error = std::io::Error;
    fn format_id(&self) -> &str {
        self.id.unwrap_or("test/runs/v1")
    }
    fn encode_commit(&self, p: &String) -> Result<Vec<u8>, Self::Error> {
        if self.fail.get() {
            return Err(err());
        }
        let mut bytes = p.as_bytes().to_vec();
        if self.changed.get() {
            bytes.push(b'!');
        }
        Ok(bytes)
    }
    fn decode_commit(&self, bytes: &[u8], _: &DecodeLimits) -> Result<String, Self::Error> {
        let s = String::from_utf8(bytes.to_vec()).map_err(|_| err())?;
        Ok(if self.permissive {
            s.trim_end().into()
        } else {
            s
        })
    }
    // Genuine across-member compression: run count, byte length, string bytes.
    fn encode_fragment(
        &self,
        _: &FragmentShape,
        payloads: &[&String],
    ) -> Result<Vec<u8>, Self::Error> {
        if self.fail.get() {
            return Err(err());
        }
        let mut out = Vec::new();
        let mut i = 0;
        while i < payloads.len() {
            let mut end = i + 1;
            while end < payloads.len() && payloads[end] == payloads[i] {
                end += 1;
            }
            out.extend_from_slice(&((end - i) as u32).to_le_bytes());
            out.extend_from_slice(&(payloads[i].len() as u32).to_le_bytes());
            out.extend_from_slice(payloads[i].as_bytes());
            i = end;
        }
        Ok(out)
    }
    fn decode_fragment(
        &self,
        shape: &FragmentShape,
        mut bytes: &[u8],
        limits: &DecodeLimits,
    ) -> Result<Vec<String>, Self::Error> {
        let mut out = Vec::new();
        let mut total = 0usize;
        while !bytes.is_empty() {
            if bytes.len() < 8 {
                return Err(err());
            }
            let count = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
            let len = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
            bytes = &bytes[8..];
            if count == 0 || count > shape.members.len() - out.len() || len > bytes.len() {
                return Err(err());
            }
            total = total
                .checked_add(count.checked_mul(len).ok_or_else(err)?)
                .ok_or_else(err)?;
            if total > limits.max_payload_bytes {
                return Err(err());
            }
            let s = std::str::from_utf8(&bytes[..len]).map_err(|_| err())?;
            out.extend((0..count).map(|_| s.to_owned()));
            bytes = &bytes[len..];
        }
        if self.bad_count.get() {
            out.pop();
        }
        Ok(out)
    }
}
fn key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}
fn author() -> Author {
    Author::try_from(key().verifying_key().as_bytes().as_slice()).unwrap()
}
fn sign(g: &mut CommitGraph<Codec>, d: &Draft) {
    g.ingest_signature(
        author(),
        d.hash,
        Signature::from(key().sign(&d.signing_bytes).to_bytes()),
    )
    .unwrap();
}
fn signed(g: &mut CommitGraph<Codec>, text: &str) -> Draft {
    let d = g.commit(author(), text.to_owned()).unwrap();
    sign(g, &d);
    d
}
fn mine(g: &mut CommitGraph<Codec>, depth: usize) -> Draft {
    for n in 0..10_000_000 {
        let p = format!("checkpoint:{n}");
        let m = g.describe_commit(author(), &p).unwrap();
        if m.hash().as_bytes().iter().take_while(|b| **b == 0).count() == depth {
            return signed(g, &p);
        }
    }
    panic!("mine")
}
fn contents(g: &CommitGraph<Codec>) -> BTreeMap<CommitHash, String> {
    let mut out = BTreeMap::new();
    g.traverse(|m, p| {
        out.insert(*m.hash(), p.clone());
        ControlFlow::Continue(())
    });
    out
}
fn inventory(g: &CommitGraph<Codec>) -> Vec<EncodedItem> {
    g.export_changes(&ExportCursor::default()).unwrap().0.added
}
fn inventory_ids(g: &CommitGraph<Codec>) -> Vec<ItemId> {
    inventory(g).iter().map(|item| item.metadata.id()).collect()
}
fn fragment(g: &mut CommitGraph<Codec>) -> EncodedItem {
    for _ in 0..100 {
        g.commit(
            author(),
            "shared long application key and repeated value".into(),
        )
        .unwrap();
    }
    let d = mine(g, 2);
    inventory(g)
        .into_iter()
        .find(|item| matches!(&item.metadata, TransportMetadata::Fragment { range, .. } if range.head == d.hash))
        .unwrap()
}
#[test]
fn incremental_signed_log_replays_without_a_separate_snapshot_api() {
    let mut source = CommitGraph::new(Codec::default());
    let root = mine(&mut source, 0);
    let (first, cursor) = source.export_changes(&ExportCursor::default()).unwrap();
    let mut log: Vec<_> = first.added.into_iter().map(|item| item.bytes).collect();
    mine(&mut source, 1);
    let expected = contents(&source);
    let (second, next) = source.export_changes(&cursor).unwrap();
    assert!(second.retired.contains(&ItemId::Commit(root.hash)));
    // Append-only storage retains superseded records; a retried write may duplicate
    // records. Neither requires another transfer format or a serialized cursor.
    for _ in 0..2 {
        log.extend(second.added.iter().map(|item| item.bytes.clone()));
    }
    let draft = source.commit(author(), "unsigned work".into()).unwrap();
    assert!(source.export_changes(&next).unwrap().0.added.is_empty());

    let mut restored = CommitGraph::new(Codec::default());
    for bytes in &log {
        assert!(restored.ingest(bytes).unwrap().rejected.is_empty());
    }
    assert_eq!(restored.pending_items(), 0);
    assert_eq!(contents(&restored), expected);
    assert!(!restored.has_commit(&draft.hash));
    assert_eq!(inventory_ids(&restored), inventory_ids(&source));

    // Establish the restored destination's baseline, then resume incremental saves.
    let (_, baseline) = restored.export_changes(&ExportCursor::default()).unwrap();
    assert!(restored.export_changes(&baseline).unwrap().0.added.is_empty());
    mine(&mut restored, 0);
    let (resumed, _) = restored.export_changes(&baseline).unwrap();
    assert_eq!(resumed.added.len(), 1);
    log.extend(resumed.added.into_iter().map(|item| item.bytes));
    let mut replayed = CommitGraph::new(Codec::default());
    for bytes in log {
        assert!(replayed.ingest(&bytes).unwrap().rejected.is_empty());
    }
    assert_eq!(replayed.pending_items(), 0);
    assert_eq!(contents(&replayed), contents(&restored));
}

#[test]
fn standalone_identity_drafts_roundtrip_and_owned_exports() {
    let mut g = CommitGraph::new(Codec::default());
    let d = g.commit(author(), "hello".into()).unwrap();
    assert!(!g.has_commit(&d.hash));
    assert!(
        g.export_changes(&ExportCursor::default())
            .unwrap()
            .0
            .added
            .is_empty()
    );
    let expected = CommitMeta::new(CommitContents {
        author: author(),
        parents: vec![],
        payload: BlobHash::of_contents(b"hello"),
    });
    assert_eq!(d.hash, *expected.hash());
    assert_eq!(d.signing_bytes, *expected.hash().as_bytes());
    sign(&mut g, &d);
    let item = inventory(&g).remove(0);
    assert_eq!(item.metadata.id(), ItemId::Commit(d.hash));
    assert_eq!(
        item.bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        include_str!("fixtures/codec-commit-v1.hex").trim()
    );
    let mut peer = CommitGraph::new(Codec::default());
    peer.ingest(&item.bytes).unwrap();
    assert_eq!(contents(&g), contents(&peer));
    signed(&mut g, "later");
    let mut late = CommitGraph::new(Codec::default());
    late.ingest(&item.bytes).unwrap();
    assert_eq!(contents(&late), contents(&peer));
}
#[test]
fn fragments_compress_roundtrip_and_keep_cursors_retryable() {
    let codec = Codec::default();
    let mut g = CommitGraph::new(codec.clone());
    let item = fragment(&mut g);
    assert!(
        item.bytes.len() < 500,
        "complete 101-member envelope: {}",
        item.bytes.len()
    );
    assert_eq!(
        item.bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        include_str!("fixtures/codec-fragment-v1.hex").trim()
    );
    let canonical: usize = contents(&g).values().map(String::len).sum();
    assert!(canonical > 4000);
    let mut peer = CommitGraph::new(Codec::default());
    peer.ingest(&item.bytes).unwrap();
    assert_eq!(contents(&g), contents(&peer));
    assert_eq!(inventory_ids(&g), inventory_ids(&peer));
    let initial = ExportCursor::default();
    codec.fail.set(true);
    assert_eq!(g.export_changes(&initial).unwrap_err().code(), "CodecError");
    codec.fail.set(false);
    let (changes, next) = g.export_changes(&initial).unwrap();
    assert_eq!(changes.added.len(), 1);
    assert!(g.export_changes(&next).unwrap().0.added.is_empty());
    let replay = g.export_changes(&initial).unwrap().0;
    assert_eq!(changes.added[0].bytes, replay.added[0].bytes);
    let old = changes.added[0].metadata.id();
    signed(&mut g, "new after captured batch");
    assert!(!g.export_changes(&next).unwrap().0.added.is_empty());
    assert!(
        g.export_changes(&next)
            .unwrap()
            .0
            .retired
            .iter()
            .all(|id| id != &old)
    );
    let bad = Codec::default();
    bad.bad_count.set(true);
    let mut receiver = CommitGraph::new(bad);
    assert_eq!(
        receiver.ingest(&item.bytes).unwrap_err().code(),
        "InvalidEncoding"
    );
    assert!(contents(&receiver).is_empty());
    assert_eq!(receiver.pending_items(), 0);
}
#[test]
fn reverse_delivery_and_encoded_merge() {
    let mut g = CommitGraph::new(Codec::default());
    let a = mine(&mut g, 1);
    let first = g.export_changes(&ExportCursor::default()).unwrap();
    for _ in 0..8 {
        signed(&mut g, "same");
    }
    let b = mine(&mut g, 1);
    let tail = g.export_changes(&first.1).unwrap().0;
    let mut peer = CommitGraph::new(Codec::default());
    for item in tail.added.iter().rev() {
        peer.ingest(&item.bytes).unwrap();
    }
    assert!(peer.pending_items() > 0);
    assert!(!peer.has_commit(&b.hash));
    for item in first.0.added {
        peer.ingest(&item.bytes).unwrap();
    }
    assert_eq!(peer.pending_items(), 0);
    assert!(peer.has_commit(&a.hash));
    assert!(peer.has_commit(&b.hash));
    assert_eq!(contents(&g), contents(&peer));
    let mut merged = CommitGraph::new(Codec::default());
    merged.merge(&peer).unwrap();
    assert_eq!(contents(&merged), contents(&peer));
    let mut wrong = CommitGraph::new(Codec {
        id: Some("other"),
        ..Codec::default()
    });
    assert_eq!(wrong.merge(&g).unwrap_err().error.code(), "WrongCodec");
    assert!(contents(&wrong).is_empty());
}
#[test]
fn noncanonical_payload_mutable_codec_and_limits_fail_atomically() {
    let codec = Codec::default();
    let mut g = CommitGraph::new(codec.clone());
    signed(&mut g, "hello ");
    let bytes = inventory(&g).remove(0).bytes;
    let mut strict = CommitGraph::new(Codec {
        permissive: true,
        ..Codec::default()
    });
    assert_eq!(
        strict.ingest(&bytes).unwrap_err().code(),
        "NonCanonicalPayload"
    );
    codec.changed.set(true);
    assert_eq!(
        g.export_changes(&ExportCursor::default()).unwrap_err().code(),
        "PayloadHashMismatch"
    );
    let mut limited = CommitGraph::with_limits(
        Codec::default(),
        DecodeLimits {
            max_payload_bytes: 2,
            ..DecodeLimits::default()
        },
    );
    assert_eq!(limited.ingest(&bytes).unwrap_err().code(), "LimitExceeded");
    assert!(contents(&limited).is_empty());
    assert_eq!(limited.pending_items(), 0);
    limited.set_decode_limits(DecodeLimits::default());
    limited.ingest(&bytes).unwrap();
    assert_eq!(contents(&limited), contents(&g));
    assert!(contents(&strict).is_empty());
    let mut tiny = CommitGraph::with_limits(
        Codec::default(),
        DecodeLimits {
            max_item_bytes: 8,
            ..DecodeLimits::default()
        },
    );
    assert_eq!(tiny.ingest(&bytes).unwrap_err().code(), "LimitExceeded");
}
#[test]
fn growing_cover_remains_exportable_and_receiver_can_retry_with_larger_budget() {
    let limits = DecodeLimits {
        max_members: 1,
        ..DecodeLimits::default()
    };
    let mut g = CommitGraph::with_limits(Codec::default(), limits.clone());
    let root = mine(&mut g, 0);
    let root_item = inventory(&g).remove(0);
    let (before, cursor) = g.export_changes(&ExportCursor::default()).unwrap();
    assert_eq!(before.added.len(), 1);
    assert_eq!(before.added[0].bytes, root_item.bytes);
    let head = mine(&mut g, 1);
    let (after, _) = g.export_changes(&cursor).unwrap();
    assert_eq!(after.added.len(), 1);
    assert!(after.retired.contains(&ItemId::Commit(root.hash)));
    assert!(matches!(
        after.added[0].metadata,
        TransportMetadata::Fragment { .. }
    ));
    let item = inventory(&g).remove(0);
    assert_eq!(item.bytes, after.added[0].bytes);
    assert_eq!(
        g.export_changes(&ExportCursor::default())
            .unwrap()
            .0
            .added
            .len(),
        1
    );
    assert!(!inventory_ids(&g).contains(&ItemId::Commit(root.hash))); // old entry retired

    let mut peer = CommitGraph::with_limits(Codec::default(), limits);
    assert_eq!(
        peer.ingest(&item.bytes).unwrap_err().code(),
        "LimitExceeded"
    );
    assert!(contents(&peer).is_empty());
    assert_eq!(peer.pending_items(), 0);
    assert!(inventory(&peer).is_empty());
    peer.set_decode_limits(DecodeLimits {
        max_members: 2,
        ..DecodeLimits::default()
    });
    assert_eq!(peer.decode_limits().max_members, 2);
    peer.ingest(&item.bytes).unwrap();
    assert!(peer.has_commit(&root.hash) && peer.has_commit(&head.hash));
    assert_eq!(contents(&peer), contents(&g));
}

#[test]
fn payload_and_envelope_budgets_are_inbound_only() {
    let small = DecodeLimits {
        max_item_bytes: 8,
        max_payload_bytes: 2,
        max_parents: 0,
        max_signatures: 0,
        ..DecodeLimits::default()
    };
    let mut local = CommitGraph::with_limits(Codec::default(), small.clone());
    let payload = "large local data".repeat(1000);
    let prepared = local.describe_commit(author(), &payload).unwrap();
    let draft = signed(&mut local, &payload);
    assert_eq!(*prepared.hash(), draft.hash);
    let item = inventory(&local).remove(0);
    assert!(item.bytes.len() > small.max_item_bytes);
    assert_eq!(
        local
            .export_changes(&ExportCursor::default())
            .unwrap()
            .0
            .added[0]
            .bytes,
        item.bytes
    );
    let mut peer = CommitGraph::with_limits(Codec::default(), small);
    assert_eq!(
        peer.ingest(&item.bytes).unwrap_err().code(),
        "LimitExceeded"
    );
    peer.set_decode_limits(DecodeLimits {
        max_payload_bytes: 2,
        ..DecodeLimits::default()
    });
    assert!(matches!(
        peer.ingest(&item.bytes),
        Err(Error::LimitExceeded("canonical payload bytes"))
    ));
    assert!(contents(&peer).is_empty());
    assert_eq!(peer.pending_items(), 0);
    peer.set_decode_limits(DecodeLimits::default());
    peer.ingest(&item.bytes).unwrap();
    assert_eq!(contents(&peer), contents(&local));
    // The same policy applies to a fragment with references and a large total
    // canonical body; signature and parent decode budgets do not block export.
    mine(&mut local, 1);
    let changes = local.export_changes(&ExportCursor::default()).unwrap().0;
    let mut fresh = CommitGraph::new(Codec::default());
    for item in changes.added {
        fresh.ingest(&item.bytes).unwrap();
    }
    assert_eq!(contents(&fresh), contents(&local));
}

fn unhex(text: &str) -> Vec<u8> {
    text.trim()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn fixed_fragment_authentication_truncations_and_resource_budgets() {
    let bytes = unhex(include_str!("fixtures/codec-fragment-v1.hex"));
    let mut peer = CommitGraph::new(Codec::default());
    for end in 0..bytes.len() {
        assert!(peer.ingest(&bytes[..end]).is_err());
    }
    for i in 0..bytes.len() {
        let mut tampered = bytes.clone();
        tampered[i] ^= 0x40;
        assert!(peer.ingest(&tampered).is_err(), "fragment tamper at {i}");
    }
    assert!(contents(&peer).is_empty());
    assert_eq!(peer.pending_items(), 0);
    for limits in [
        DecodeLimits {
            max_members: 100,
            ..DecodeLimits::default()
        },
        DecodeLimits {
            max_parents: 99,
            ..DecodeLimits::default()
        },
        DecodeLimits {
            max_signatures: 0,
            ..DecodeLimits::default()
        },
    ] {
        let mut limited = CommitGraph::with_limits(Codec::default(), limits);
        assert_eq!(limited.ingest(&bytes).unwrap_err().code(), "LimitExceeded");
        assert!(contents(&limited).is_empty());
        assert_eq!(limited.pending_items(), 0);
    }
    peer.ingest(&bytes).unwrap();
    assert_eq!(contents(&peer).len(), 101);
    let exported = peer.export_changes(&ExportCursor::default()).unwrap().0;
    assert_eq!(exported.added[0].bytes, bytes);
}

#[test]
fn mixed_author_fork_roundtrips_original_evidence() {
    let mut a = CommitGraph::new(Codec::default());
    signed(&mut a, "root");
    let mut b = a.clone();
    signed(&mut a, "alice branch");
    let bob_key = SigningKey::from_bytes(&[9; 32]);
    let bob = Author::try_from(bob_key.verifying_key().as_bytes().as_slice()).unwrap();
    let d = b.commit(bob, "bob branch".into()).unwrap();
    b.ingest_signature(
        bob,
        d.hash,
        Signature::from(bob_key.sign(&d.signing_bytes).to_bytes()),
    )
    .unwrap();
    a.merge(&b).unwrap();
    assert_eq!(a.heads().count(), 2);
    mine(&mut a, 1);
    let mut receiver = CommitGraph::new(Codec::default());
    let changes = a.export_changes(&ExportCursor::default()).unwrap().0;
    assert!(
        changes
            .added
            .iter()
            .any(|i| matches!(i.metadata, TransportMetadata::Fragment { .. }))
    );
    for item in changes.added.iter().rev() {
        receiver.ingest(&item.bytes).unwrap();
    }
    assert_eq!(contents(&a), contents(&receiver));
    assert_eq!(receiver.pending_items(), 0);
    assert_eq!(inventory_ids(&a), inventory_ids(&receiver));
}

// A statelessly valid fragment whose omitted external boundary only becomes
// invalid when its dependency arrives (regression from the core review).
fn queued_invalid(dependency: CommitHash) -> (Vec<u8>, ItemId) {
    fn put(out: &mut Vec<u8>, n: usize) {
        use hexane::Codec as _;
        out.extend(hexane::Leb128::encode_unsigned(n as u64));
    }
    fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
        put(out, bytes.len());
        out.extend(bytes);
    }
    let (payload, meta) = (0..)
        .find_map(|i| {
            let payload = format!("queued:{i}");
            let meta = CommitMeta::new(CommitContents {
                parents: vec![dependency],
                author: author(),
                payload: BlobHash::of_contents(payload.as_bytes()),
            });
            (meta.hash().as_bytes()[0] == 0 && meta.hash().as_bytes()[1] != 0)
                .then_some((payload, meta))
        })
        .unwrap();
    let id = ItemId::Fragment(FragmentRange {
        head: *meta.hash(),
        boundary: Default::default(),
    });
    let mut out = b"STKF\x01\x01".to_vec();
    blob(&mut out, b"test/runs/v1");
    out.extend(meta.hash().as_bytes());
    out.extend([0, 1, 1]); // empty boundary, one member, one author
    out.extend(author().as_slice());
    out.push(1);
    out.extend(dependency.as_bytes());
    for column in [&[0x7f, 0][..], &[0x7f, 1], &[1], &[], &[0x7f, 0]] {
        blob(&mut out, column);
    }
    out.push(1);
    blob(&mut out, &[0x7f, 0]);
    blob(&mut out, &[0x7f, 0]);
    out.extend(key().sign(meta.hash().as_bytes()).to_bytes());
    let body = Codec::default()
        .encode_fragment(&FragmentShape { members: vec![] }, &[&payload])
        .unwrap();
    blob(&mut out, &body);
    (out, id)
}

#[test]
fn merge_error_retains_earlier_admissions_deferred_rejections_and_pending() {
    let codec = Codec::default();
    let mut source = CommitGraph::new(codec.clone());
    let a = mine(&mut source, 0);
    // Loose items export in hash order. Only the first fits the receiver budget.
    let b = (0..)
        .find_map(|i| {
            let text = format!("{}:{i}", "x".repeat(1000));
            let m = source.describe_commit(author(), &text).unwrap();
            (m.hash().as_bytes()[0] != 0 && m.hash() > &a.hash).then_some(text)
        })
        .unwrap();
    let b = signed(&mut source, &b);
    let items = source
        .export_changes(&ExportCursor::default())
        .unwrap()
        .0
        .added;
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].metadata.id(), ItemId::Commit(a.hash));
    let mut dest = CommitGraph::with_limits(
        Codec::default(),
        DecodeLimits {
            max_payload_bytes: 100,
            ..DecodeLimits::default()
        },
    );
    let (queued, rejected_id) = queued_invalid(a.hash);
    dest.ingest(&queued).unwrap();
    // An unrelated queued item must still be represented by the final count.
    dest.ingest(&queued_invalid([0x55; 32].into()).0).unwrap();
    assert_eq!(dest.pending_items(), 2);

    let wrong = CommitGraph::new(Codec {
        id: Some("other"),
        ..Codec::default()
    });
    let error = dest.merge(&wrong).unwrap_err();
    assert_eq!(error.error.code(), "WrongCodec");
    assert_eq!(
        error.report,
        IngestReport {
            pending: 2,
            ..IngestReport::default()
        }
    );
    codec.fail.set(true);
    let error = dest.merge(&source).unwrap_err();
    assert_eq!(error.error.code(), "CodecError");
    assert_eq!(
        error.report,
        IngestReport {
            pending: 2,
            ..IngestReport::default()
        }
    );
    assert!(!dest.has_commit(&a.hash));
    codec.fail.set(false);

    let error = dest.merge(&source).unwrap_err();
    assert_eq!(error.error.code(), "LimitExceeded");
    assert!(std::error::Error::source(&error).is_some());
    assert!(error.to_string().contains("LimitExceeded"));
    assert_eq!(error.report.admitted, vec![ItemId::Commit(a.hash)]);
    assert_eq!(error.report.rejected.len(), 1);
    assert_eq!(error.report.rejected[0].item, rejected_id);
    assert_eq!(error.report.pending, 1);
    assert_eq!(dest.pending_items(), 1);
    assert!(dest.has_commit(&a.hash));
    assert!(!dest.has_commit(&b.hash));
    // Replaying the successful ingest cannot recover the rejection; it must be
    // returned by the failed merge rather than discarded.
    assert!(dest.ingest(&items[0].bytes).unwrap().rejected.is_empty());
    dest.set_decode_limits(DecodeLimits::default());
    let report = dest.merge(&source).unwrap();
    assert!(dest.has_commit(&b.hash));
    assert_eq!(report.pending, 1);
}

#[test]
fn all_truncations_trailing_wrong_format_and_authenticated_tampering() {
    let mut g = CommitGraph::new(Codec::default());
    signed(&mut g, "hello");
    let item = inventory(&g).remove(0);
    let mut peer = CommitGraph::new(Codec::default());
    for n in 0..item.bytes.len() {
        assert!(peer.ingest(&item.bytes[..n]).is_err(), "truncation {n}");
    }
    let mut extra = item.bytes.clone();
    extra.push(0);
    assert!(peer.ingest(&extra).is_err());
    let mut version = item.bytes.clone();
    version[4] = 2;
    assert_eq!(peer.ingest(&version).unwrap_err().code(), "InvalidEncoding");
    let mut format = item.bytes.clone();
    format[7] ^= 1;
    assert_eq!(peer.ingest(&format).unwrap_err().code(), "WrongCodec");
    // Every one-byte perturbation of a standalone envelope must fail: no
    // unauthenticated author/payload/signature field can change the meaning.
    for i in 0..item.bytes.len() {
        let mut tampered = item.bytes.clone();
        tampered[i] ^= 0x40;
        assert!(peer.ingest(&tampered).is_err(), "tamper at {i}");
    }
    assert!(contents(&peer).is_empty());
    assert_eq!(peer.pending_items(), 0);
}
