//! Driving `Admission` the way a foreign host does: requests are executed by
//! hand, possibly late, out of order, or wrongly.
mod support;
use std::collections::HashMap;
use stockfoil::*;
use support::*;

/// A host with blob storage and per-operation decoders, under test control.
#[derive(Default)]
struct Host {
    blobs: HashMap<BlobHash, Vec<u8>>,
    next_id: PayloadId,
    read_limit: Option<usize>,
    decode_limit: Option<usize>,
}

impl Host {
    fn store(&mut self, bytes: Vec<u8>) -> BlobHash {
        let hash = BlobHash::of_contents(&bytes);
        self.blobs.insert(hash, bytes);
        hash
    }

    /// Execute a request for stored item `item` into owned reply data.
    fn execute(
        &mut self,
        item: &BlobHash,
        request: Request<'_>,
        decoder: &mut Option<ChunkDecoder>,
    ) -> Owned {
        match request {
            Request::Read { offset, max } => {
                let bytes = &self.blobs[item];
                let max = self.read_limit.map_or(max, |l| l.min(max));
                let start = (offset as usize).min(bytes.len());
                let end = (start + max).min(bytes.len());
                Owned::Read(bytes[start..end].to_vec(), end == bytes.len())
            }
            Request::Decode {
                meta,
                input,
                eof,
                capacity,
            } => {
                let d = decoder.get_or_insert_with(|| ChunkDecoder::new(meta.len()));
                let mut out = vec![0; self.decode_limit.map_or(capacity, |l| l.min(capacity))];
                match d.step(input, eof, &mut out) {
                    Ok(p) => {
                        out.truncate(p.produced);
                        Owned::Decoded(p.consumed, out, p.status)
                    }
                    Err(e) => Owned::Failed(e.to_string()),
                }
            }
            Request::Bind { meta } => {
                let ids = (0..meta.len() as u64).map(|i| self.next_id + i).collect();
                self.next_id += meta.len() as u64;
                Owned::Bound(ids)
            }
        }
    }
}

#[derive(Debug, Clone)]
enum Owned {
    Read(Vec<u8>, bool),
    Decoded(usize, Vec<u8>, DecodeStatus),
    Bound(Vec<PayloadId>),
    Failed(String),
}

impl Owned {
    fn reply(&self) -> Reply<'_> {
        match self {
            Owned::Read(b, eof) => Reply::Read {
                bytes: b,
                eof: *eof,
            },
            Owned::Decoded(c, o, s) => Reply::Decoded {
                consumed: *c,
                output: o,
                status: *s,
            },
            Owned::Bound(ids) => Reply::Bound(ids),
            Owned::Failed(m) => Reply::Failed(m),
        }
    }
}

enum Outcome {
    Done(Receipt),
    Waiting(CommitHash),
    Failed(Error),
}

/// Drive the admission of `item` to completion, executing requests immediately.
fn run(history: &mut History, op: &mut Admission, host: &mut Host, item: &BlobHash) -> Outcome {
    let mut decoder = None;
    loop {
        let pending = match op.next() {
            Next::Continue => continue,
            Next::Done(r) => return Outcome::Done(r.clone()),
            Next::Failed(e) => return Outcome::Failed(e.clone()),
            Next::NeedHistory | Next::Waiting(_) => match history.advance(op) {
                Next::Done(r) => return Outcome::Done(r.clone()),
                Next::Failed(e) => return Outcome::Failed(e.clone()),
                Next::Waiting(h) => return Outcome::Waiting(h),
                _ => None,
            },
            Next::Request(t, r) => Some((t, host.execute(item, r, &mut decoder))),
            Next::AwaitReply => {
                let (t, r) = op.request().unwrap();
                Some((t, host.execute(item, r, &mut decoder)))
            }
        };
        if let Some((ticket, owned)) = pending
            && let Err(ReplyError::Invalid(e)) = op.reply(ticket, owned.reply())
        {
            return Outcome::Failed(e);
        }
    }
}

fn options() -> ReceiveOptions {
    ReceiveOptions::new(format())
}

/// Items exported from a graph with two signed commits and one depth-1 fragment.
fn items() -> (Vec<(ItemId, Vec<u8>)>, Vec<CommitHash>) {
    let alice = key(1);
    let mut a = graph();
    let mut tip = signed(&mut a, &alice, vec![], b"root");
    let mut hashes = vec![tip];
    for i in 0..3 {
        tip = signed(
            &mut a,
            &alice,
            vec![tip],
            format!("member {i}").repeat(50).as_bytes(),
        );
        hashes.push(tip);
    }
    let payload = payload_at_depth(&alice, &[tip], 1, "head");
    tip = signed(&mut a, &alice, vec![tip], &payload);
    hashes.push(tip);
    let after = signed(&mut a, &alice, vec![tip], b"after");
    hashes.push(after);
    let plan = a.export(&ExportCursor::new());
    let items = (0..plan.len())
        .map(|i| {
            let mut bytes = vec![];
            a.write_item(&plan, i, &mut bytes).unwrap();
            (plan.id(i).unwrap().clone(), bytes)
        })
        .collect();
    (items, hashes)
}

fn fragment(items: &[(ItemId, Vec<u8>)]) -> Vec<u8> {
    items
        .iter()
        .find(|(id, _)| matches!(id, ItemId::Fragment(_)))
        .unwrap()
        .1
        .clone()
}

#[test]
fn admits_with_one_byte_reads_and_one_byte_decoder_output() {
    let (items, hashes) = items();
    let mut history = History::new();
    let mut host = Host {
        read_limit: Some(1),
        decode_limit: Some(1),
        ..Default::default()
    };
    let bytes = fragment(&items);
    let item = host.store(bytes.clone());
    let mut op = history.receive(options());
    let Outcome::Done(receipt) = run(&mut history, &mut op, &mut host, &item) else {
        panic!("not admitted")
    };
    assert_eq!(receipt.members.len(), 5);
    assert!(hashes[..5].iter().all(|h| history.is_admitted(h)));
    let counts = op.counts();
    assert_eq!(counts.bytes_read, bytes.len() as u64);
    let canonical: usize = (0..3)
        .map(|i| format!("member {i}").repeat(50).len())
        .sum::<usize>()
        + 4;
    assert!(counts.canonical_bytes >= canonical as u64);
}

#[test]
fn stale_duplicate_and_wrong_kind_replies_do_not_disturb_the_operation() {
    let (items, _) = items();
    let mut history = History::new();
    let mut host = Host::default();
    let item = host.store(fragment(&items));
    let mut op = history.receive(options());
    let mut decoder = None;
    let Next::Request(first, request) = op.next() else {
        panic!()
    };
    let read = host.execute(&item, request, &mut decoder);
    // Wrong kind for the outstanding read.
    assert_eq!(
        op.reply(first, Reply::Bound(&[1, 2, 3])).unwrap_err(),
        ReplyError::WrongKind
    );
    // Unknown ticket.
    assert_eq!(
        op.reply(Ticket::from_raw(u64::MAX), read.reply()).unwrap_err(),
        ReplyError::Stale
    );
    op.reply(first, read.reply()).unwrap();
    // Duplicate of an already answered request.
    assert_eq!(
        op.reply(first, read.reply()).unwrap_err(),
        ReplyError::Stale
    );
    assert!(matches!(
        run(&mut history, &mut op, &mut host, &item),
        Outcome::Done(_)
    ));
}

#[test]
fn local_work_and_other_admissions_proceed_while_a_request_is_pending() {
    let (items, _) = items();
    let mut history = History::new();
    let mut host = Host::default();
    let ids: Vec<_> = items.iter().map(|(_, b)| host.store(b.clone())).collect();
    let mut ops: Vec<_> = ids.iter().map(|_| history.receive(options())).collect();
    // Start every operation and leave its first read outstanding.
    let mut pending = vec![];
    for (op, item) in ops.iter_mut().zip(&ids) {
        let Next::Request(t, r) = op.next() else {
            panic!()
        };
        pending.push((t, host.execute(item, r, &mut None)));
    }
    // Local edits happen meanwhile.
    let bob = key(2);
    let local = history.begin_commit(author(&bob), vec![]).unwrap().finish();
    history.publish_commit(local.clone(), 1000).unwrap();
    history
        .sign(author(&bob), local.hash, sign(&bob, &local.hash))
        .unwrap();
    // Answer in reverse order, then drive each, retrying waiting ones.
    for (op, (t, owned)) in ops.iter_mut().zip(&pending).rev() {
        op.reply(*t, owned.reply()).unwrap();
    }
    let mut done = 0;
    for _round in 0..ops.len() {
        for (op, item) in ops.iter_mut().zip(&ids) {
            if let Outcome::Done(_) = run(&mut history, op, &mut host, item) {
                done += 1;
            }
        }
        if done == ops.len() {
            break;
        }
        done = 0;
    }
    assert!(ops.iter_mut().all(|op| matches!(op.next(), Next::Done(_))));
    assert!(history.is_admitted(&local.hash));
}

#[test]
fn waiting_costs_nothing_until_the_epoch_changes() {
    let (items, hashes) = items();
    let mut history = History::new();
    let mut host = Host::default();
    let after = items
        .iter()
        .find(|(id, _)| *id == ItemId::Commit(hashes[5]))
        .unwrap();
    let item = host.store(after.1.clone());
    let mut op = history.receive(options());
    assert!(
        matches!(run(&mut history, &mut op, &mut host, &item), Outcome::Waiting(h) if h == hashes[4])
    );
    let read = op.counts().bytes_read;
    assert!(matches!(history.advance(&mut op), Next::Waiting(_)));
    assert_eq!(op.counts().bytes_read, read);
    // An unrelated delivery changes the epoch; the dependency is still missing.
    let carol = key(3);
    let p = history
        .begin_commit(author(&carol), vec![])
        .unwrap()
        .finish();
    history.publish_commit(p, 7).unwrap();
    assert!(matches!(history.advance(&mut op), Next::Waiting(_)));
    let frag = host.store(fragment(&items));
    let mut first = history.receive(options());
    assert!(matches!(
        run(&mut history, &mut first, &mut host, &frag),
        Outcome::Done(_)
    ));
    assert!(matches!(
        run(&mut history, &mut op, &mut host, &item),
        Outcome::Done(_)
    ));
    assert!(history.is_admitted(&hashes[5]));
}

#[test]
fn wrong_history_is_rejected_without_failing_the_operation() {
    let (items, _) = items();
    let mut history = History::new();
    let mut other = History::new();
    let mut host = Host::default();
    let item = host.store(fragment(&items));
    let mut op = history.receive(options());
    let mut decoder = None;
    loop {
        match op.next() {
            Next::NeedHistory => break,
            Next::Request(t, r) => {
                let owned = host.execute(&item, r, &mut decoder);
                op.reply(t, owned.reply()).unwrap();
            }
            Next::Continue => {}
            other => panic!("{other:?}"),
        }
    }
    assert!(matches!(
        other.advance(&mut op),
        Next::Failed(Error::WrongHistory)
    ));
    assert!(other.is_empty());
    assert!(matches!(
        run(&mut history, &mut op, &mut host, &item),
        Outcome::Done(_)
    ));
}

#[test]
fn dropping_an_operation_leaves_history_untouched() {
    let (items, _) = items();
    let history = History::new();
    let mut host = Host::default();
    let item = host.store(fragment(&items));
    let mut op = history.receive(options());
    let Next::Request(t, r) = op.next() else {
        panic!()
    };
    let owned = host.execute(&item, r, &mut None);
    op.reply(t, owned.reply()).unwrap();
    drop(op);
    assert!(history.is_empty());
    assert_eq!(history.epoch(), 0);
}

#[test]
fn tampered_items_are_rejected() {
    let (items, _) = items();
    let bytes = fragment(&items);
    let header = wire::header_len(&bytes, &Limits::default())
        .unwrap()
        .unwrap();

    let mut host = Host::default();
    let mut history = History::new();

    // A corrupted signature.
    let mut corrupt = bytes.clone();
    corrupt[header - 1] ^= 1;
    let item = host.store(corrupt);
    let mut op = history.receive(options());
    assert!(matches!(
        run(&mut history, &mut op, &mut host, &item),
        Outcome::Failed(Error::InvalidSignature { .. })
    ));

    // A corrupted payload byte changes the reconstructed identity.
    let mut corrupt = bytes.clone();
    corrupt[header + 10] ^= 1;
    let item = host.store(corrupt);
    let mut op = history.receive(options());
    assert!(matches!(
        run(&mut history, &mut op, &mut host, &item),
        Outcome::Failed(Error::InvalidItem(_) | Error::InvalidSignature { .. } | Error::Codec(_))
    ));

    // Trailing bytes after the body.
    let mut trailing = bytes.clone();
    trailing.push(0);
    let item = host.store(trailing);
    let mut op = history.receive(options());
    assert!(matches!(
        run(&mut history, &mut op, &mut host, &item),
        Outcome::Failed(_)
    ));

    // Truncated body: the codec sees EOF early.
    let item = host.store(bytes[..bytes.len() - 3].to_vec());
    let mut op = history.receive(options());
    assert!(matches!(
        run(&mut history, &mut op, &mut host, &item),
        Outcome::Failed(Error::Codec(_))
    ));
    assert!(history.is_empty());
}

#[test]
fn unexpected_format_fails_before_any_decoding() {
    let (items, _) = items();
    let history = History::new();
    let mut host = Host::default();
    let item = host.store(fragment(&items));
    let mut op = history.receive(ReceiveOptions::new(FormatId::new("other").unwrap()));
    loop {
        match op.next() {
            Next::Request(t, r) => {
                assert!(matches!(r, Request::Read { .. }), "decoder must not run");
                let owned = host.execute(&item, r, &mut None);
                let _ = op.reply(t, owned.reply());
            }
            Next::Failed(e) => {
                assert_eq!(*e, Error::WrongFormat);
                break;
            }
            Next::Continue => {}
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn decoder_protocol_violations_fail_the_operation() {
    let (items, _) = items();
    let bytes = fragment(&items);
    let violations: Vec<Box<dyn Fn(usize, usize) -> Owned>> = vec![
        Box::new(|offered, _| Owned::Decoded(offered + 1, vec![], DecodeStatus::Working)),
        Box::new(|offered, _| {
            Owned::Decoded(offered.saturating_sub(1), vec![], DecodeStatus::NeedInput)
        }),
        Box::new(|_, _| Owned::Decoded(0, vec![], DecodeStatus::Working)),
        Box::new(|_, cap| Owned::Decoded(0, vec![0; cap + 1], DecodeStatus::Working)),
        Box::new(|offered, _| Owned::Decoded(offered, vec![], DecodeStatus::Finished)),
    ];
    for (i, violation) in violations.iter().enumerate() {
        let history = History::new();
        let mut host = Host::default();
        let item = host.store(bytes.clone());
        let mut op = history.receive(options());
        let mut decoder = None;
        let failed = loop {
            match op.next() {
                Next::Request(
                    t,
                    Request::Decode {
                        input, capacity, ..
                    },
                ) => {
                    let owned = violation(input.len(), capacity);
                    if let Err(ReplyError::Invalid(e)) = op.reply(t, owned.reply()) {
                        break e;
                    }
                }
                Next::Request(t, r) => {
                    let owned = host.execute(&item, r, &mut decoder);
                    op.reply(t, owned.reply()).unwrap();
                }
                Next::Failed(e) => break e.clone(),
                Next::Continue => {}
                other => panic!("violation {i}: {other:?}"),
            }
        };
        assert!(
            matches!(failed, Error::Protocol(_) | Error::InvalidItem(_)),
            "violation {i}: {failed:?}"
        );
        assert!(history.is_empty());
    }
}

#[test]
fn host_failures_are_terminal_and_keep_counts() {
    let (items, _) = items();
    let history = History::new();
    let mut host = Host::default();
    let item = host.store(fragment(&items));
    let mut op = history.receive(options());
    let Next::Request(t, r) = op.next() else {
        panic!()
    };
    let owned = host.execute(&item, r, &mut None);
    op.reply(t, owned.reply()).unwrap();
    let before = op.counts();
    let Next::Request(t, _) = op.next() else {
        panic!()
    };
    assert!(matches!(
        op.reply(t, Reply::Failed("disk on fire")),
        Err(ReplyError::Invalid(Error::Codec(_)))
    ));
    assert!(matches!(op.next(), Next::Failed(Error::Codec(_))));
    assert_eq!(op.counts().bytes_read, before.bytes_read);
}

#[test]
fn bind_is_skipped_when_every_member_is_already_delivered() {
    let (items, _) = items();
    let mut history = History::new();
    let mut host = Host::default();
    let item = host.store(fragment(&items));
    let mut op = history.receive(options());
    let Outcome::Done(first) = run(&mut history, &mut op, &mut host, &item) else {
        panic!()
    };
    let mut again = history.receive(options());
    let mut binds = 0;
    let mut decoder = None;
    loop {
        match again.next() {
            Next::Request(t, r) => {
                binds += matches!(r, Request::Bind { .. }) as usize;
                let owned = host.execute(&item, r, &mut decoder);
                again.reply(t, owned.reply()).unwrap();
            }
            Next::NeedHistory => {
                let _ = history.advance(&mut again);
            }
            Next::Continue | Next::AwaitReply => {}
            Next::Done(receipt) => {
                assert!(receipt.members.iter().all(|m| !m.newly_delivered));
                assert_eq!(
                    receipt.members,
                    first
                        .members
                        .iter()
                        .map(|m| Delivered {
                            newly_delivered: false,
                            ..*m
                        })
                        .collect::<Vec<_>>()
                );
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(binds, 0);
}

#[test]
fn concurrent_admissions_of_one_item_keep_the_first_payload_ids() {
    let (items, _) = items();
    let mut history = History::new();
    let mut host = Host::default();
    let item = host.store(fragment(&items));
    let mut a = history.receive(options());
    let mut b = history.receive(options());
    // Drive both up to their bind requests.
    let mut binds = vec![];
    for op in [&mut a, &mut b] {
        let mut decoder = None;
        loop {
            match op.next() {
                Next::Request(t, r @ Request::Bind { .. }) => {
                    binds.push((t, host.execute(&item, r, &mut None)));
                    break;
                }
                Next::Request(t, r) => {
                    let owned = host.execute(&item, r, &mut decoder);
                    op.reply(t, owned.reply()).unwrap();
                }
                Next::NeedHistory => {
                    if let Next::Request(t, r) = history.advance(op) {
                        binds.push((t, host.execute(&item, r, &mut None)));
                        break;
                    }
                }
                Next::Continue => {}
                other => panic!("{other:?}"),
            }
        }
    }
    // Publish the second one first.
    b.reply(binds[1].0, binds[1].1.reply()).unwrap();
    let Next::Done(rb) = history.advance(&mut b) else {
        panic!()
    };
    let rb = rb.clone();
    a.reply(binds[0].0, binds[0].1.reply()).unwrap();
    let Next::Done(ra) = history.advance(&mut a) else {
        panic!()
    };
    assert!(rb.members.iter().all(|m| m.newly_delivered));
    assert!(ra.members.iter().all(|m| !m.newly_delivered));
    for (x, y) in ra.members.iter().zip(&rb.members) {
        assert_eq!(x.payload, y.payload);
    }
}

#[test]
fn replies_for_another_operation_are_stale() {
    let (items, _) = items();
    let history = History::new();
    let mut host = Host::default();
    let ids: Vec<_> = items.iter().take(2).map(|(_, b)| host.store(b.clone())).collect();
    let mut a = history.receive(options());
    let mut b = history.receive(options());
    let Next::Request(ta, ra) = a.next() else { panic!() };
    let reply_a = host.execute(&ids[0], ra, &mut None);
    let Next::Request(tb, _) = b.next() else { panic!() };
    assert_ne!(ta, tb);
    assert_eq!(b.reply(ta, reply_a.reply()).unwrap_err(), ReplyError::Stale);
    a.reply(ta, reply_a.reply()).unwrap();
}
