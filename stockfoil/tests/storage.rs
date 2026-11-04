//! Storage keys are opaque, and payloads read back from storage are checked
//! against their commits rather than trusted.
mod support;
use std::collections::BTreeMap;
use std::io::{self, Write};
use stockfoil::*;
use support::*;

/// Stands in for an encrypting store: keys are its own counters, unrelated to
/// the contents, and bytes are transformed at rest.
#[derive(Default)]
struct SealedStorage {
    blobs: Vec<Vec<u8>>,
    payloads: Vec<PayloadLocation<u64>>,
}

const MASK: u8 = 0x5a;

impl SealedStorage {
    /// Flip one plaintext byte of a stored blob; negative offsets count from the end.
    fn corrupt(&mut self, key: u64, offset: isize) {
        let blob = &mut self.blobs[key as usize];
        let at = offset.rem_euclid(blob.len() as isize) as usize;
        blob[at] ^= 1;
    }
}

impl Storage for SealedStorage {
    type Key = u64;
    type Writer = Vec<u8>;
    fn create(&mut self) -> io::Result<Vec<u8>> {
        Ok(Vec::new())
    }
    fn finish(&mut self, writer: Vec<u8>) -> io::Result<u64> {
        self.blobs.push(writer.iter().map(|b| b ^ MASK).collect());
        Ok(self.blobs.len() as u64 - 1)
    }
    fn read_at(&mut self, key: &u64, offset: u64, out: &mut [u8]) -> io::Result<usize> {
        let blob = self
            .blobs
            .get(*key as usize)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown blob"))?;
        let start = (offset as usize).min(blob.len());
        let n = out.len().min(blob.len() - start);
        for (o, b) in out.iter_mut().zip(&blob[start..start + n]) {
            *o = b ^ MASK;
        }
        Ok(n)
    }
    fn bind(&mut self, location: PayloadLocation<u64>) -> io::Result<PayloadId> {
        self.payloads.push(location);
        Ok(self.payloads.len() as PayloadId - 1)
    }
    fn location(&self, id: PayloadId) -> io::Result<PayloadLocation<u64>> {
        self.payloads
            .get(id as usize)
            .copied()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown payload ID"))
    }
}

type SealedGraph = Graph<ChunkCodec, SealedStorage>;

fn sealed() -> SealedGraph {
    Graph::new(ChunkCodec::default(), SealedStorage::default())
}

/// A sealed graph holding `root`, three members, and a depth-1 head, all
/// signed, so that they export as one fragment.
fn fragment_source() -> (SealedGraph, Vec<CommitHash>) {
    let alice = key(1);
    let mut g = sealed();
    let commit = |g: &mut SealedGraph, parents, payload: &[u8]| {
        let prepared = g.commit(author(&alice), parents, payload).unwrap();
        g.sign(author(&alice), prepared.hash, sign(&alice, &prepared.hash))
            .unwrap();
        prepared.hash
    };
    let mut hashes = vec![commit(&mut g, vec![], b"root")];
    for i in 0..3 {
        let tip = *hashes.last().unwrap();
        hashes.push(commit(&mut g, vec![tip], format!("member {i}").as_bytes()));
    }
    let tip = *hashes.last().unwrap();
    let payload = payload_at_depth(&alice, &[tip], 1, "head");
    hashes.push(commit(&mut g, vec![tip], &payload));
    (g, hashes)
}

/// Send every planned item from `from` to `to`; returns the receiver's keys.
fn send(from: &mut SealedGraph, to: &mut SealedGraph) -> Vec<u64> {
    let plan = from.export(&ExportCursor::new());
    (0..plan.len())
        .map(|i| {
            let mut bytes = Vec::new();
            from.write_item(&plan, i, &mut bytes).unwrap();
            let mut writer = to.storage_mut().create().unwrap();
            writer.write_all(&bytes).unwrap();
            let key = to.storage_mut().finish(writer).unwrap();
            to.admit(&key).unwrap();
            key
        })
        .collect()
}

#[test]
fn items_are_admitted_and_read_through_opaque_keys() {
    let (mut a, hashes) = fragment_source();
    let mut b = sealed();
    let keys = send(&mut a, &mut b);
    assert_eq!(keys.len(), 1, "one fragment");
    for h in &hashes {
        assert!(b.history().is_admitted(h));
        assert_eq!(b.payload(h).unwrap(), a.payload(h).unwrap());
    }
    // Several members in one decoding pass.
    let wanted = BTreeMap::from([(1, hashes[1]), (3, hashes[3])]);
    let mut got: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
    b.decode_item(&keys[0], &wanted, |member, bytes| {
        got.entry(member).or_default().extend_from_slice(bytes);
        Ok(())
    })
    .unwrap();
    assert_eq!(got[&1], b"member 0");
    assert_eq!(got[&3], b"member 2");
    // And they re-export from the receiver.
    let mut c = sealed();
    send(&mut b, &mut c);
    assert_eq!(
        c.payload(&hashes[4]).unwrap(),
        a.payload(&hashes[4]).unwrap()
    );
}

#[test]
fn corrupt_stored_payloads_fail_reads() {
    let (mut a, hashes) = fragment_source();
    let mut b = sealed();
    let keys = send(&mut a, &mut b);
    let head = hashes[4];

    // A received item: the last body byte before the terminator belongs to
    // the head's payload. Other members still read correctly.
    b.storage_mut().corrupt(keys[0], -5);
    assert_eq!(b.payload(&head), Err(Error::CorruptPayload(head)));
    assert_eq!(b.payload(&hashes[0]).unwrap(), b"root");

    // A local commit's canonical blob.
    let id = a.history().lookup(&head).unwrap().payload;
    let PayloadLocation::Canonical(blob) = a.storage().location(id).unwrap() else {
        panic!("local payloads are stored canonically")
    };
    a.storage_mut().corrupt(blob, 0);
    assert_eq!(a.payload(&head), Err(Error::CorruptPayload(head)));
}

#[test]
fn decoded_members_must_be_the_wanted_commits() {
    let (mut a, hashes) = fragment_source();
    let mut b = sealed();
    let keys = send(&mut a, &mut b);
    let mut visit = |_: usize, _: &[u8]| Ok(());
    // Member 0 is not the head's payload.
    assert_eq!(
        b.decode_item(&keys[0], &BTreeMap::from([(0, hashes[4])]), &mut visit),
        Err(Error::CorruptPayload(hashes[4]))
    );
    // The item has no member 99.
    assert_eq!(
        b.decode_item(&keys[0], &BTreeMap::from([(99, hashes[0])]), &mut visit),
        Err(Error::CorruptPayload(hashes[0]))
    );
    // Checking an undelivered commit is an error of its own.
    let unknown = CommitHash::from([7; 32]);
    assert_eq!(
        b.decode_item(&keys[0], &BTreeMap::from([(0, unknown)]), &mut visit),
        Err(Error::UnknownCommit(unknown))
    );
    assert_eq!(b.payload(&unknown), Err(Error::UnknownCommit(unknown)));
}

#[test]
fn decode_item_state_machine_streams_and_stops_at_last_wanted_member() {
    let (mut a, hashes) = fragment_source();
    let plan = a.export(&ExportCursor::new());
    let mut item = Vec::new();
    a.write_item(&plan, 0, &mut item).unwrap();

    // Drive DecodeItem directly with 1-byte reads and 1-byte decode capacity.
    let mut options = ReceiveOptions::new(format());
    options.read_size = 1;
    options.decode_capacity = 1;
    let mut op = a.history().decode_item(options, [(1, hashes[1])]).unwrap();

    // Stale and wrong-kind replies are rejected without disturbing the operation.
    let DecodeNext::Request(ticket, Request::Read { offset, max }) = op.next() else {
        panic!("expected initial read request")
    };
    assert!(matches!(op.next(), DecodeNext::AwaitReply));
    assert_eq!(
        op.reply(
            Ticket::from_raw(ticket.get() + 1),
            Reply::Read {
                bytes: &item[..1],
                eof: false
            }
        ),
        Err(ReplyError::Stale)
    );
    assert_eq!(
        op.reply(ticket, Reply::Bound(&[0])),
        Err(ReplyError::WrongKind)
    );
    op.reply(
        ticket,
        Reply::Read {
            bytes: &item[offset as usize..(offset as usize + max).min(item.len())],
            eof: offset as usize + max >= item.len(),
        },
    )
    .unwrap();

    let mut decoder: Option<ChunkDecoder> = None;
    let mut collected = Vec::new();
    loop {
        match op.next() {
            DecodeNext::Chunk { member, bytes } => {
                assert_eq!(member, 1);
                collected.extend_from_slice(bytes);
            }
            DecodeNext::Done => break,
            DecodeNext::Request(ticket, Request::Read { offset, max }) => {
                let start = (offset as usize).min(item.len());
                let end = (start + max).min(item.len());
                op.reply(
                    ticket,
                    Reply::Read {
                        bytes: &item[start..end],
                        eof: end == item.len(),
                    },
                )
                .unwrap();
            }
            DecodeNext::Request(
                ticket,
                Request::Decode {
                    meta,
                    input,
                    eof,
                    capacity,
                },
            ) => {
                let d = decoder.get_or_insert_with(|| ChunkDecoder::new(meta.len()));
                let mut out = vec![0u8; capacity];
                let p = d.step(input, eof, &mut out).unwrap();
                op.reply(
                    ticket,
                    Reply::Decoded {
                        consumed: p.consumed,
                        output: &out[..p.produced],
                        status: p.status,
                    },
                )
                .unwrap();
            }
            other => panic!("unexpected state: {other:?}"),
        }
    }
    assert_eq!(collected, b"member 0");
    // Stopped right after member 1 without reading the rest of the 5-member item.
    assert!(
        (op.counts().bytes_read as usize) < item.len(),
        "read {} of {} bytes",
        op.counts().bytes_read,
        item.len()
    );
}
