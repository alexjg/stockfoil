use super::*;
use crate::meta::Evidence;
use crate::{
    Author, BundleMeta, DecodeStatus, FormatId, MemberShape, ParentRef, ReceiveOptions, Reply,
    Signature,
};
use ed25519_dalek::{Signer, SigningKey};

struct Item {
    hash: CommitHash,
    bytes: Vec<u8>,
}

fn format() -> FormatId {
    FormatId::new("queue-test/raw-v1").unwrap()
}

fn item(sender: &mut History, parents: &[CommitHash], tag: u64) -> Item {
    let key = SigningKey::from_bytes(&[1; 32]);
    let author = Author::from(&key);
    for nonce in 0u64.. {
        let payload = [tag.to_le_bytes(), nonce.to_le_bytes()].concat();
        let mut builder = sender.begin_commit(author, parents.to_vec()).unwrap();
        builder.update(&payload);
        let prepared = builder.finish();
        if crate::meta::depth(&prepared.hash) != 0 {
            continue;
        }
        let hash = prepared.hash;
        sender.publish_commit(prepared, tag).unwrap();
        let meta = BundleMeta {
            id: ItemId::Commit(hash),
            members: vec![MemberShape {
                author,
                parents: parents.iter().copied().map(ParentRef::External).collect(),
            }],
            evidence: vec![Evidence {
                author,
                member: 0,
                signature: Signature::from(key.sign(hash.as_bytes())),
            }],
        };
        let mut bytes = crate::wire::encode_header(&meta, &format());
        bytes.extend(payload);
        return Item { hash, bytes };
    }
    unreachable!()
}

fn verified(history: &History, item: &Item) -> Admission {
    let mut op = history.receive(ReceiveOptions::new(format()));
    let mut ended = false;
    loop {
        match op.next() {
            Next::Continue => continue,
            Next::NeedHistory => return op,
            Next::Request(..) | Next::AwaitReply => {}
            other => panic!("unexpected verification state: {other:?}"),
        }
        let (ticket, request) = op.request().unwrap();
        match request {
            Request::Read { offset, max } => {
                let start = offset as usize;
                let end = start.saturating_add(max).min(item.bytes.len());
                op.reply(
                    ticket,
                    Reply::Read {
                        bytes: &item.bytes[start..end],
                        eof: end == item.bytes.len(),
                    },
                )
                .unwrap();
            }
            Request::Decode { input, .. } => {
                let bytes = input.to_vec();
                let status = if ended {
                    DecodeStatus::Finished
                } else {
                    DecodeStatus::EndMember
                };
                ended = true;
                op.reply(
                    ticket,
                    Reply::Decoded {
                        consumed: bytes.len(),
                        output: &bytes,
                        status,
                    },
                )
                .unwrap();
            }
            Request::Bind { .. } => panic!("not yet placed"),
        }
    }
}

fn park<T>(
    queue: &mut Queue<T>,
    history: &mut History,
    item: &Item,
    context: T,
) -> Option<Evicted<T>> {
    let mut op = verified(history, item);
    let Next::Waiting(dep) = history.advance(&mut op) else {
        panic!("expected a missing dependency")
    };
    queue.push(dep, context, op).unwrap()
}

fn admit(history: &mut History, item: &Item) {
    let mut op = verified(history, item);
    let Next::Request(ticket, Request::Bind { .. }) = history.advance(&mut op) else {
        panic!("expected bind")
    };
    op.reply(ticket, Reply::Bound(&[0])).unwrap();
    assert!(matches!(history.advance(&mut op), Next::Done(_)));
}

fn finish_one(queue: &mut Queue<usize>, history: &mut History) -> (usize, ItemId) {
    let QueueNext::Bind {
        context,
        mut op,
        ticket,
        member_count,
    } = queue.poll(history).expect("ready item")
    else {
        panic!("expected a new commit to bind");
    };
    assert_eq!(member_count, 1);
    op.reply(ticket, Reply::Bound(&[0])).unwrap();
    let Next::Done(receipt) = history.advance(&mut op) else {
        panic!("expected publication")
    };
    (context, receipt.id.clone())
}

#[test]
fn cap_evicts_oldest_across_different_dependencies_and_returns_context() {
    let (mut sender, mut receiver) = (History::new(), History::new());
    let parents: Vec<_> = (0..3).map(|tag| item(&mut sender, &[], tag)).collect();
    let children: Vec<_> = parents
        .iter()
        .enumerate()
        .map(|(i, p)| item(&mut sender, &[p.hash], i as u64 + 10))
        .collect();
    let mut queue = Queue::with_capacity(2);
    assert_eq!(Queue::<()>::new().capacity(), 1024);
    assert!(park(&mut queue, &mut receiver, &children[0], 100).is_none());
    assert!(park(&mut queue, &mut receiver, &children[1], 200).is_none());
    let evicted = park(&mut queue, &mut receiver, &children[2], 300).unwrap();
    assert_eq!(evicted.id, ItemId::Commit(children[0].hash));
    assert_eq!(evicted.context, 100);
    assert_eq!(queue.len(), 2);
    assert_eq!(queue.waiting.len(), 2);
    assert!(!receiver.is_admitted(&children[0].hash));
    for parent in &parents {
        admit(&mut receiver, parent);
    }
    let mut contexts = vec![
        finish_one(&mut queue, &mut receiver).0,
        finish_one(&mut queue, &mut receiver).0,
    ];
    contexts.sort();
    assert_eq!(contexts, [200, 300]);
    assert!(queue.poll(&mut receiver).is_none());
    assert!(queue.is_empty());
    assert!(queue.waiting.is_empty());
}

#[test]
fn changing_dependencies_preserves_original_parking_age() {
    let (mut sender, mut receiver) = (History::new(), History::new());
    let p = item(&mut sender, &[], 0);
    let q = item(&mut sender, &[], 1);
    let old = item(&mut sender, &[p.hash, q.hash], 2);
    let young = item(&mut sender, &[old.hash], 3);
    let newest = item(&mut sender, &[young.hash], 4);
    let mut queue = Queue::with_capacity(2);
    assert!(park(&mut queue, &mut receiver, &old, 0).is_none());
    assert!(park(&mut queue, &mut receiver, &young, 1).is_none());
    let missing = *queue
        .waiting
        .keys()
        .find(|h| **h == p.hash || **h == q.hash)
        .unwrap();
    admit(&mut receiver, if missing == p.hash { &p } else { &q });
    assert!(queue.poll(&mut receiver).is_none());
    let evicted = park(&mut queue, &mut receiver, &newest, 2).unwrap();
    assert_eq!(evicted.id, ItemId::Commit(old.hash));
    assert_eq!(evicted.context, 0);
    assert_eq!(queue.len(), 2);
}

#[test]
fn no_fifo_tombstones_accumulate_behind_a_permanently_missing_item() {
    let (mut sender, mut receiver) = (History::new(), History::new());
    let missing = item(&mut sender, &[], 0);
    let stuck = item(&mut sender, &[missing.hash], 1);
    let mut queue = Queue::with_capacity(2);
    assert!(park(&mut queue, &mut receiver, &stuck, 0).is_none());
    for i in 0..32 {
        let parent = item(&mut sender, &[], i + 10);
        let child = item(&mut sender, &[parent.hash], i + 100);
        assert!(park(&mut queue, &mut receiver, &child, i as usize + 1).is_none());
        admit(&mut receiver, &parent);
        assert_eq!(finish_one(&mut queue, &mut receiver).0, i as usize + 1);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.waiting.len(), 1);
        assert_eq!(queue.waiting.values().map(BTreeSet::len).sum::<usize>(), 1);
    }
}

#[test]
fn eviction_releases_only_the_admission_and_hands_context_back_for_cleanup() {
    use std::cell::Cell;
    use std::rc::Rc;
    struct Staged(Rc<Cell<usize>>);
    impl Drop for Staged {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let dropped = Rc::new(Cell::new(0));
    let (mut sender, mut receiver) = (History::new(), History::new());
    let parent = item(&mut sender, &[], 0);
    let child = item(&mut sender, &[parent.hash], 1);
    let mut queue = Queue::with_capacity(1);
    assert!(park(&mut queue, &mut receiver, &child, Staged(dropped.clone())).is_none());
    let evicted = park(&mut queue, &mut receiver, &child, Staged(dropped.clone())).unwrap();
    assert_eq!(dropped.get(), 0); // the caller still owns the evicted staging
    drop(evicted);
    assert_eq!(dropped.get(), 1);
    drop(queue);
    assert_eq!(dropped.get(), 2); // no hidden copies in either index
}

#[test]
fn zero_cap_returns_the_incoming_item_without_retaining_it() {
    let (mut sender, mut receiver) = (History::new(), History::new());
    let parent = item(&mut sender, &[], 0);
    let child = item(&mut sender, &[parent.hash], 1);
    let mut queue = Queue::with_capacity(0);
    let evicted = park(&mut queue, &mut receiver, &child, "staged payload").unwrap();
    assert_eq!(evicted.id, ItemId::Commit(child.hash));
    assert_eq!(evicted.context, "staged payload");
    assert!(queue.is_empty());
    assert!(queue.waiting.is_empty());
    assert!(!receiver.is_admitted(&child.hash));
}

#[test]
fn ready_items_release_in_causal_order_and_duplicates_need_no_binding() {
    let (mut sender, mut receiver) = (History::new(), History::new());
    let parent = item(&mut sender, &[], 0);
    let child = item(&mut sender, &[parent.hash], 1);
    let grandchild = item(&mut sender, &[child.hash], 2);
    let mut queue = Queue::with_capacity(3);
    assert!(park(&mut queue, &mut receiver, &grandchild, 2).is_none());
    assert!(park(&mut queue, &mut receiver, &child, 1).is_none());
    assert!(park(&mut queue, &mut receiver, &child, 3).is_none());
    admit(&mut receiver, &parent);
    assert_eq!(
        finish_one(&mut queue, &mut receiver),
        (1, ItemId::Commit(child.hash))
    );
    let mut done = Vec::new();
    while let Some(next) = queue.poll(&mut receiver) {
        match next {
            QueueNext::Done { context, receipt } => {
                assert_eq!(context, 3);
                assert_eq!(receipt.id, ItemId::Commit(child.hash));
                done.push(context);
            }
            QueueNext::Bind {
                context,
                mut op,
                ticket,
                ..
            } => {
                assert_eq!(context, 2);
                op.reply(ticket, Reply::Bound(&[0])).unwrap();
                assert!(matches!(receiver.advance(&mut op), Next::Done(_)));
                done.push(context);
            }
            QueueNext::Failed { error, .. } => panic!("{error}"),
        }
    }
    done.sort();
    assert_eq!(done, [2, 3]);
    assert!(queue.is_empty());
    assert!(queue.waiting.is_empty());
}

#[test]
fn invalid_push_and_age_exhaustion_do_not_evict_existing_entries() {
    let (mut sender, mut receiver) = (History::new(), History::new());
    let parent = item(&mut sender, &[], 0);
    let child = item(&mut sender, &[parent.hash], 1);
    let mut queue = Queue::with_capacity(1);
    assert!(park(&mut queue, &mut receiver, &child, 1).is_none());
    let op = receiver.receive(ReceiveOptions::new(format()));
    assert!(matches!(
        queue.push(parent.hash, 2, op),
        Err(Error::Protocol(_))
    ));
    assert_eq!(queue.len(), 1);
    queue.next = u64::MAX;
    let mut op = verified(&receiver, &child);
    let Next::Waiting(dep) = receiver.advance(&mut op) else {
        panic!("expected wait")
    };
    assert!(matches!(
        queue.push(dep, 3, op),
        Err(Error::Exhausted("queue ages"))
    ));
    assert_eq!(queue.len(), 1);
    admit(&mut receiver, &parent);
    assert_eq!(finish_one(&mut queue, &mut receiver).0, 1);
}
