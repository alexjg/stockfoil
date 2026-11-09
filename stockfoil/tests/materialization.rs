//! The planner protocol, independent states, resumability and failure handling.
mod support;
use std::collections::{BTreeMap, BTreeSet};
use stockfoil::*;
use support::*;

#[derive(Clone, Default)]
struct State {
    known: BTreeMap<CommitHash, Bytes>,
    active: BTreeSet<CommitHash>,
    resets: usize,
    retracts: usize,
    supports: bool,
    fail_checkpoint: bool,
}
impl Materializer for State {
    type Error = &'static str;
    fn apply(&mut self, _: &History, events: Vec<Event<Bytes>>) -> Result<Applied, Self::Error> {
        let mut applied = Applied::default();
        for event in events {
            match event {
                Event::Reset => {
                    self.known.clear();
                    self.active.clear();
                    self.resets += 1;
                }
                Event::Deliver {
                    hash,
                    payload,
                    active,
                    ..
                } => {
                    assert!(
                        self.known.insert(hash, payload.clone()).is_none(),
                        "duplicate first delivery"
                    );
                    if payload.starts_with(b"bad") {
                        applied.rejected.push(Rejection {
                            hash,
                            reason: "invalid test payload".into(),
                            cascade: payload.starts_with(b"bad+"),
                        });
                    } else if active {
                        self.active.insert(hash);
                    }
                }
                Event::Activate { hash } => {
                    assert!(self.known.contains_key(&hash));
                    self.active.insert(hash);
                }
                Event::Retract { hash } => {
                    self.active.remove(&hash);
                    self.retracts += 1;
                }
                Event::Payload { hash, payload } => {
                    assert_eq!(self.known.get(&hash), Some(&payload))
                }
            }
        }
        Ok(applied)
    }
    fn checkpoint(&mut self, _: &Checkpoint) -> Result<(), Self::Error> {
        if self.fail_checkpoint {
            Err("cannot persist")
        } else {
            Ok(())
        }
    }
    fn supports_visibility_changes(&self) -> bool {
        self.supports
    }
}

fn add(doc: &mut TestDocument, bytes: &[u8]) -> CommitHash {
    doc.commit(bytes, None).unwrap().hash
}

#[test]
fn delivery_and_visibility_are_separate_and_multiple_states_follow_independently() {
    let (alice, bob) = (key(1), key(2));
    let mut doc = document_for(&alice);
    let root = signed(&mut doc, &alice, vec![], b"a");
    let hidden = signed(&mut doc, &bob, vec![root], b"b");
    let descendant = signed(&mut doc, &alice, vec![hidden], b"c");
    let mut live = Materialization::new(Follow::Working, true);
    let mut pinned = Materialization::new(Follow::Pinned(vec![root]), true);
    let (mut state, mut historical) = (State::default(), State::default());
    live.update(&doc, &mut state).unwrap();
    pinned.update(&doc, &mut historical).unwrap();
    assert_eq!(state.known.len(), 3);
    assert_eq!(state.active, BTreeSet::from([root, descendant]));
    assert_eq!(historical.known.len(), 3); // out-of-view metadata is delivered too
    assert_eq!(historical.active, BTreeSet::from([root]));
    doc.set_permissions(WritePermissions::from([
        (author(&alice), WritePermission::Unbounded),
        (author(&bob), WritePermission::Unbounded),
    ]));
    let p = live.update(&doc, &mut state).unwrap();
    assert_eq!(p.delivered, 0);
    assert_eq!(p.activated, 1);
    assert!(state.active.contains(&hidden));
    assert_eq!(historical.active, BTreeSet::from([root]));
    doc.set_permissions(WritePermissions::from([(
        author(&alice),
        WritePermission::Unbounded,
    )]));
    let p = live.update(&doc, &mut state).unwrap();
    assert_eq!(p.retracted, 1);
    assert_eq!(state.active, BTreeSet::from([root, descendant]));
    assert!(!live.update(&doc, &mut state).unwrap().changed());
}

#[test]
fn open_documents_materialize_edits_from_authors_they_have_never_listed() {
    let (alice, stranger) = (key(1), key(2));
    let mut doc = document_for(&alice);
    doc.set_permissions(WritePermissions::new().with_unlisted(WritePermission::Unbounded));
    let root = signed(&mut doc, &alice, vec![], b"a");
    let edit = signed(&mut doc, &stranger, vec![root], b"b");
    let mut live = Materialization::new(Follow::Working, false);
    let mut state = State::default();
    live.update(&doc, &mut state).unwrap();
    assert_eq!(state.active, BTreeSet::from([root, edit]));
    // The local author is unlisted too, and may still edit.
    let draft = live
        .change(&mut doc, &mut state, |_| Ok::<_, &str>(Some(b"c".to_vec())))
        .unwrap()
        .unwrap();
    assert!(doc.ledger().contains(&draft.hash));
}

#[test]
fn unsupported_visibility_changes_reset_then_redeliver_in_batches() {
    let mut doc = document();
    let first = add(&mut doc, b"a");
    add(&mut doc, b"b");
    let mut m = Materialization::new(Follow::Working, false);
    let mut state = State::default();
    m.update(&doc, &mut state).unwrap();
    doc.set_permissions(WritePermissions::from([(
        doc.author(),
        WritePermission::Bounded(vec![first]),
    )]));
    while m.step(&doc, &mut state, 1).unwrap() != Step::Current {}
    assert_eq!(state.resets, 1);
    assert_eq!(state.known.len(), 2);
    assert_eq!(state.active, BTreeSet::from([first]));
}

#[test]
fn resuming_after_a_reset_does_not_reset_again_or_lose_rejections() {
    for cascade in [false, true] {
        let mut doc = document();
        add(
            &mut doc,
            if cascade {
                b"bad+ parent"
            } else {
                b"good parent"
            },
        );
        add(&mut doc, b"good child");
        let mut m = Materialization::new(Follow::Working, false);
        let mut state = State::default();
        if !cascade {
            m.update(&doc, &mut state).unwrap();
            doc.set_permissions(WritePermissions::new());
        }
        let budget = if cascade { 1024 } else { 1 };
        assert_eq!(m.step(&doc, &mut state, budget).unwrap(), Step::More);
        assert_eq!(state.resets, 1);
        assert!(state.known.is_empty());
        assert!(m.checkpoint().is_partial());
        let saved = Checkpoint::from_bytes(&m.checkpoint().to_bytes()).unwrap();
        let mut resumed = Materialization::resume(saved, false);
        resumed.update(&doc, &mut state).unwrap();
        assert_eq!(state.resets, 1);
        assert_eq!(state.known.len(), 2);
        assert!(state.active.is_empty());
        assert_eq!(resumed.checkpoint().rejected().len(), usize::from(cascade));
    }
}

#[test]
fn wanted_payload_rounds_hold_the_checkpoint_and_batches_are_owned_tokens() {
    let mut doc = document();
    let hash = add(&mut doc, b"a");
    let ledger = doc.ledger();
    let mut m = Materialization::new(Follow::Working, true);
    let saved = m.checkpoint().to_bytes();
    let batch = m.plan(ledger, 1).unwrap().unwrap();
    let resolved = batch.resolve(|id| id + 100);
    assert_eq!(resolved.len(), 1); // resolve does not consume completion token
    assert!(matches!(m.plan(ledger, 1), Err(Error::Protocol(_))));
    let requested = m
        .complete(ledger, batch, Applied::default().want(hash))
        .unwrap()
        .unwrap();
    assert_eq!(m.checkpoint().to_bytes(), saved);
    assert!(
        matches!(requested.events(), [Event::Payload { hash: h, payload }] if *h == hash && Some(*payload) == ledger.payload_id(&hash))
    );
    assert!(
        m.complete(ledger, requested, Applied::default())
            .unwrap()
            .is_none()
    );
    assert!(m.is_current(ledger));

    let mut a = Materialization::new(Follow::Working, true);
    let mut b = Materialization::new(Follow::Working, true);
    let token_a = a.plan(ledger, 1).unwrap().unwrap();
    let token_b = b.plan(ledger, 1).unwrap().unwrap();
    assert_eq!(
        a.complete(ledger, token_b, Applied::default()).unwrap_err(),
        Error::StaleBatch
    );
    a.complete(ledger, token_a, Applied::default()).unwrap();
    let other = document();
    assert_eq!(a.plan(other.ledger(), 1).unwrap_err(), Error::WrongHistory);
}

#[test]
fn follow_changes_during_payload_rounds_keep_rejections_until_publication() {
    let mut doc = document();
    let hash = add(&mut doc, b"bad payload");
    let ledger = doc.ledger();
    let mut m = Materialization::new(Follow::Working, true);
    let batch = m.plan(ledger, 1).unwrap().unwrap();
    m.set_follow(Follow::Signed);
    let mut applied = Applied::default().reject(hash, "invalid").want(hash);
    applied.rejected[0].cascade = true;
    let payload = m.complete(ledger, batch, applied).unwrap().unwrap();
    assert!(m.checkpoint().heads().is_empty());
    assert_eq!(m.checkpoint().rejected().len(), 0);
    m.set_follow(Follow::Pinned(vec![hash]));
    // The logical batch remains pending across payload rounds. Repeating a
    // rejection must preserve its earlier cascading strength.
    assert!(
        m.complete(
            ledger,
            payload,
            Applied::default().reject(hash, "still invalid")
        )
        .unwrap()
        .is_none()
    );
    assert_eq!(m.follow(), &Follow::Pinned(vec![hash]));
    assert_eq!(m.checkpoint().heads(), &[hash]);
    assert!(!m.checkpoint().is_partial());
    assert_eq!(
        m.checkpoint().rejected().collect::<Vec<_>>(),
        vec![(hash, true)]
    );
    let restored = stockfoil::Checkpoint::from_bytes(&m.checkpoint().to_bytes()).unwrap();
    assert_eq!(&restored, m.checkpoint());
}

#[test]
fn cloned_tokens_cannot_complete_again_or_replace_a_follow_up() {
    let mut doc = document();
    let hash = add(&mut doc, b"a");
    let ledger = doc.ledger();
    let mut m = Materialization::new(Follow::Working, true);
    let saved = m.checkpoint().clone();
    let batch = m.plan(ledger, 1).unwrap().unwrap();
    assert!(!batch.is_rebuild());
    assert_eq!(batch.events().len(), batch.len());
    let stale = batch.clone();
    let requested = m
        .complete(ledger, batch, Applied::default().want(hash))
        .unwrap()
        .unwrap();
    assert_eq!(
        m.complete(ledger, stale, Applied::default()).unwrap_err(),
        Error::StaleBatch
    );
    assert_eq!(m.checkpoint(), &saved);
    let replay = requested.clone();
    m.complete(ledger, requested, Applied::default()).unwrap();
    let completed = m.checkpoint().clone();
    assert_eq!(
        m.complete(ledger, replay, Applied::default()).unwrap_err(),
        Error::StaleBatch
    );
    assert_eq!(m.checkpoint(), &completed);
}

#[test]
fn rejections_outside_delivery_are_rejected_without_consuming_the_token() {
    let mut doc = document();
    add(&mut doc, b"a");
    let mut m = Materialization::new(Follow::Working, true);
    let saved = m.checkpoint().clone();
    let batch = m.plan(doc.ledger(), 1).unwrap().unwrap();
    let arbitrary = CommitHash::from([0xff; 32]);
    assert!(matches!(
        m.complete(
            doc.ledger(),
            batch.clone(),
            Applied::default().reject(arbitrary, "not in batch")
        ),
        Err(Error::Protocol(_))
    ));
    assert_eq!(m.checkpoint(), &saved);
    m.complete(doc.ledger(), batch, Applied::default()).unwrap();
    assert!(m.is_current(doc.ledger()));
}

#[test]
fn rejected_commits_never_reactivate_and_cascades_correct_same_batch_descendants() {
    for supports in [false, true] {
        let mut doc = document();
        let bad = add(&mut doc, b"bad+ parent");
        let child = add(&mut doc, b"good child");
        let mut m = Materialization::new(Follow::Working, supports);
        let mut state = State {
            supports,
            ..State::default()
        };
        m.update(&doc, &mut state).unwrap();
        assert!(state.active.is_empty());
        assert_eq!(state.known.len(), 2);
        assert_eq!(m.checkpoint().rejected().collect::<Vec<_>>(), [(bad, true)]);
        if supports {
            assert_eq!(state.retracts, 1);
        } else {
            assert_eq!(state.resets, 1);
        }
        let saved = Checkpoint::from_bytes(&m.checkpoint().to_bytes()).unwrap();
        let mut m = Materialization::resume(saved, supports);
        doc.set_permissions(WritePermissions::new());
        m.update(&doc, &mut state).unwrap();
        doc.set_permissions(WritePermissions::from([(
            doc.author(),
            WritePermission::Unbounded,
        )]));
        m.update(&doc, &mut state).unwrap();
        assert!(state.active.is_empty());
        let next = add(&mut doc, b"later descendant");
        m.update(&doc, &mut state).unwrap();
        assert!(state.known.contains_key(&child) && state.known.contains_key(&next));
        assert!(state.active.is_empty());
    }
}

#[test]
fn non_cascading_rejection_does_not_hide_valid_descendants() {
    let mut doc = document();
    let bad = add(&mut doc, b"bad independent");
    let good = add(&mut doc, b"good descendant");
    let mut m = Materialization::new(Follow::Working, true);
    let mut state = State::default();
    m.update(&doc, &mut state).unwrap();
    assert_eq!(state.active, BTreeSet::from([good]));
    assert_eq!(
        m.checkpoint().rejected().collect::<Vec<_>>(),
        [(bad, false)]
    );
}

#[test]
fn every_partial_checkpoint_resumes_after_reordered_arrivals_and_later_edits() {
    let signer = key(1);
    let mut original = document_for(&signer);
    for i in 0..12u8 {
        let draft = original.commit(vec![i], None).unwrap();
        original
            .attach_signature(&draft, sign(&signer, &draft.hash))
            .unwrap();
    }
    let export = original.export(&ExportCursor::new());
    let items: Vec<_> = (0..export.len())
        .map(|i| export.item(i).unwrap().1)
        .collect();
    for stop in 1..12 {
        let mut m = Materialization::new(Follow::Working, true);
        let mut state = State::default();
        for _ in 0..stop {
            m.step(&original, &mut state, 1).unwrap();
        }
        let checkpoint = Checkpoint::from_bytes(&m.checkpoint().to_bytes()).unwrap();
        assert!(checkpoint.is_partial());
        let mut rebuilt = document_for(&key(9));
        for item in items.iter().rev() {
            rebuilt.receive(item).unwrap();
        }
        rebuilt.set_permissions(WritePermissions::from([
            (author(&signer), WritePermission::Unbounded),
            (rebuilt.author(), WritePermission::Unbounded),
        ]));
        let later = add(&mut rebuilt, b"later");
        let mut resumed = Materialization::resume(checkpoint, true);
        while resumed.checkpoint().is_partial() {
            resumed.step(&rebuilt, &mut state, 2).unwrap();
        }
        assert_eq!(state.known.len(), 12);
        assert!(!state.known.contains_key(&later));
        resumed.update(&rebuilt, &mut state).unwrap();
        assert_eq!(state.known.len(), 13);
        assert!(state.active.contains(&later));
    }
}

#[test]
fn impossible_resume_counts_fail_before_touching_state() {
    let mut doc = document();
    add(&mut doc, b"a");
    add(&mut doc, b"b");
    let mut m = Materialization::new(Follow::Working, true);
    let mut state = State::default();
    m.step(&doc, &mut state, 1).unwrap();
    let mut bytes = m.checkpoint().to_bytes();
    let count_offset = bytes.len() - 8;
    bytes[count_offset..].copy_from_slice(&u64::MAX.to_le_bytes());
    let checkpoint = Checkpoint::from_bytes(&bytes).unwrap();
    let mut restored = Materialization::resume(checkpoint, true);
    assert!(matches!(
        restored.plan(doc.ledger(), 1),
        Err(Error::Malformed(_))
    ));
    assert_eq!(state.known.len(), 1);
}

#[test]
fn preflight_errors_do_not_poison_untouched_state() {
    let mut doc = document();
    add(&mut doc, b"a");
    let mut m = Materialization::new(Follow::Working, true);
    let mut state = State::default();
    m.update(&doc, &mut state).unwrap();
    let other = document();
    assert_eq!(
        m.step(&other, &mut state, 1).unwrap_err(),
        DriveError::Stockfoil(Error::WrongHistory)
    );
    assert!(matches!(
        m.step(&doc, &mut state, 0),
        Err(DriveError::Stockfoil(Error::Limit(_)))
    ));
    assert!(!m.update(&doc, &mut state).unwrap().changed());
    assert_eq!(state.known.len(), 1);
}

#[test]
fn unwritable_change_reports_the_reason_without_mutating_or_poisoning_state() {
    let mut doc = document();
    for (follow, permission, message) in [
        (
            Follow::Signed,
            Some(WritePermission::Unbounded),
            "cannot edit a signed-only view",
        ),
        (
            Follow::Working,
            Some(WritePermission::Denied),
            "local author requires unbounded write permission to edit",
        ),
        (
            Follow::Working,
            Some(WritePermission::Bounded(vec![])),
            "local author requires unbounded write permission to edit",
        ),
        (
            Follow::Working,
            None,
            "local author requires unbounded write permission to edit",
        ),
    ] {
        let permissions = permission.into_iter().map(|p| (doc.author(), p)).collect();
        doc.set_permissions(permissions);
        let mut m = Materialization::new(follow, true);
        let mut state = State::default();
        m.update(&doc, &mut state).unwrap();
        let saved = m.checkpoint().clone();
        let result = m.change(
            &mut doc,
            &mut state,
            |_| -> Result<Option<Vec<u8>>, &'static str> {
                panic!("edit must not run for a non-writable materialization")
            },
        );
        assert_eq!(
            result.unwrap_err(),
            DriveError::Stockfoil(Error::Protocol(message))
        );
        assert_eq!(m.checkpoint(), &saved);
        assert!(state.known.is_empty());
        assert!(!m.update(&doc, &mut state).unwrap().changed());
    }
}

#[derive(Default)]
struct Counter {
    value: u64,
    deliveries: usize,
}
impl Materializer for Counter {
    type Error = &'static str;
    fn apply(&mut self, _: &History, events: Vec<Event<Bytes>>) -> Result<Applied, Self::Error> {
        for event in events {
            match event {
                Event::Reset => self.value = 0,
                Event::Deliver {
                    payload, active, ..
                } => {
                    self.deliveries += 1;
                    if active {
                        self.value += u64::from_le_bytes(payload.as_ref().try_into().unwrap());
                    }
                }
                _ => {}
            }
        }
        Ok(Applied::default())
    }
}

#[test]
fn local_change_is_not_echoed_and_pinned_changes_create_a_branch() {
    let mut doc = document();
    let base = add(&mut doc, &1u64.to_le_bytes());
    let side = add(&mut doc, &2u64.to_le_bytes());
    let mut m = Materialization::new(Follow::Pinned(vec![base]), false);
    let mut state = Counter::default();
    m.update(&doc, &mut state).unwrap();
    assert_eq!(state.value, 1);
    let deliveries = state.deliveries;
    let draft = m
        .change(&mut doc, &mut state, |state| {
            state.value += 7;
            Ok::<_, &'static str>(Some(7u64.to_le_bytes().to_vec()))
        })
        .unwrap()
        .unwrap();
    assert_eq!(draft.parents, [base]);
    assert_eq!(m.follow(), &Follow::Pinned(vec![draft.hash]));
    assert!(m.is_current(doc.ledger()));
    m.update(&doc, &mut state).unwrap();
    assert_eq!(state.value, 8);
    assert_eq!(state.deliveries, deliveries);
    assert!(!doc.ledger().is_ancestor(&side, &draft.hash));
}

#[test]
fn failed_checkpoints_poison_the_driver_and_preserve_last_checkpoint() {
    let mut doc = document();
    add(&mut doc, b"a");
    let mut m = Materialization::new(Follow::Working, true);
    let saved = m.checkpoint().clone();
    let mut state = State {
        fail_checkpoint: true,
        ..State::default()
    };
    assert!(matches!(
        m.step(&doc, &mut state, 1),
        Err(DriveError::Checkpoint("cannot persist"))
    ));
    assert_eq!(m.checkpoint(), &saved);
    assert_eq!(m.plan(doc.ledger(), 1).unwrap_err(), Error::Poisoned);
}

#[test]
fn a_missing_payload_prevents_the_whole_batch_from_reaching_the_application() {
    let mut doc = document();
    let root = add(&mut doc, b"stored");
    // Raw Ledger commits may legitimately refer to storage outside Document.
    let mut builder = doc.ledger().begin_commit(Some(vec![root])).unwrap();
    builder.update(b"external");
    let prepared = builder.finish();
    doc.ledger_mut().commit(prepared, u64::MAX).unwrap();
    let mut m = Materialization::new(Follow::Working, true);
    let saved = m.checkpoint().clone();
    struct NoCalls;
    impl Materializer for NoCalls {
        type Error = &'static str;
        fn apply(&mut self, _: &History, _: Vec<Event<Bytes>>) -> Result<Applied, Self::Error> {
            panic!("a partially resolved batch must not be applied")
        }
        fn checkpoint(&mut self, _: &Checkpoint) -> Result<(), Self::Error> {
            panic!("failed resolution must not publish a checkpoint")
        }
    }
    assert_eq!(
        m.step(&doc, &mut NoCalls, 2),
        Err(DriveError::Stockfoil(Error::Storage(
            "unknown in-memory payload ID".into()
        )))
    );
    assert_eq!(m.checkpoint(), &saved);
    assert_eq!(m.plan(doc.ledger(), 1).unwrap_err(), Error::Poisoned);
}

#[test]
fn stateful_batch_resolution_keeps_event_order_visibility_and_the_completion_token() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let child = add(&mut doc, b"child");
    let mut m = Materialization::new(Follow::Pinned(vec![root]), true);
    let batch = m.plan(doc.ledger(), 2).unwrap().unwrap();
    let mut visited = Vec::new();
    let events = batch.resolve::<String>(|id| {
        visited.push(id);
        format!("call {}", visited.len())
    });
    assert_eq!(
        visited,
        [
            doc.ledger().payload_id(&root).unwrap(),
            doc.ledger().payload_id(&child).unwrap()
        ]
    );
    assert!(matches!(&events[..], [
        Event::Deliver { hash: first, parents: first_parents, payload: first_bytes, active: true, .. },
        Event::Deliver { hash: second, parents: second_parents, payload: second_bytes, active: false, .. },
    ] if *first == root && first_parents.is_empty() && first_bytes == "call 1"
        && *second == child && second_parents == &[root] && second_bytes == "call 2"));
    assert!(
        m.complete(doc.ledger(), batch, Applied::default())
            .unwrap()
            .is_none()
    );
    assert!(m.is_current(doc.ledger()));
}

#[test]
fn payload_mapping_preserves_metadata_and_returns_the_original_error() {
    let hash = CommitHash::from([1; 32]);
    let who = author(&key(2));
    let parents = vec![CommitHash::from([3; 32])];
    // Captured values need not implement Copy or Clone; this remains FnOnce.
    let replacement = String::from("resolved");
    let event = Event::Deliver {
        hash,
        author: who,
        parents: parents.clone(),
        payload: 4u64,
        active: false,
    }
    .map_payload::<String>(|id| {
        assert_eq!(id, 4);
        replacement
    });
    assert_eq!(
        event,
        Event::Deliver {
            hash,
            author: who,
            parents,
            payload: "resolved".into(),
            active: false
        }
    );
    struct Failure(Box<u8>);
    let failure = Failure(Box::new(17));
    let address = &*failure.0 as *const u8;
    let result = Event::Payload {
        hash,
        payload: 8u64,
    }
    .try_map_payload::<Bytes, Failure>(|id| {
        assert_eq!(id, 8);
        Err(failure)
    });
    let Err(returned) = result else {
        panic!("resolver failed")
    };
    assert_eq!(&*returned.0 as *const u8, address);
    assert_eq!(*returned.0, 17);
    for event in [
        Event::Reset,
        Event::Activate { hash },
        Event::Retract { hash },
    ] {
        let result = event.try_map_payload::<u8, ()>(|_: u64| panic!("no payload to resolve"));
        assert!(result.is_ok());
    }
}

#[test]
fn step_finishes_all_feedback_before_checkpointing_once_and_clean_current_is_a_noop() {
    struct Traced {
        hash: CommitHash,
        trace: Vec<&'static str>,
        saved: Option<Checkpoint>,
    }
    impl Materializer for Traced {
        type Error = &'static str;
        fn apply(
            &mut self,
            _: &History,
            events: Vec<Event<Bytes>>,
        ) -> Result<Applied, Self::Error> {
            match &events[..] {
                [
                    Event::Deliver {
                        hash,
                        active: true,
                        payload,
                        ..
                    },
                ] => {
                    assert_eq!(*hash, self.hash);
                    assert_eq!(&**payload, b"value");
                    self.trace.push("deliver");
                }
                [Event::Payload { hash, payload }] => {
                    assert_eq!(*hash, self.hash);
                    assert_eq!(&**payload, b"value");
                    self.trace.push("payload");
                }
                _ => panic!("unexpected callback events"),
            }
            let mut report = Applied::default();
            if self.trace.len() < 3 {
                report.wanted.push(self.hash);
            }
            Ok(report)
        }
        fn checkpoint(&mut self, checkpoint: &Checkpoint) -> Result<(), Self::Error> {
            assert_eq!(self.trace, ["deliver", "payload", "payload"]);
            assert_eq!(checkpoint.heads(), &[self.hash]);
            assert!(!checkpoint.is_partial());
            self.trace.push("checkpoint");
            self.saved = Some(checkpoint.clone());
            Ok(())
        }
    }
    let mut doc = document();
    let hash = add(&mut doc, b"value");
    let mut driver = Materialization::new(Follow::Working, true);
    let mut state = Traced {
        hash,
        trace: Vec::new(),
        saved: None,
    };
    // The existing named type arguments and concrete function-pointer use remain valid.
    let step: fn(
        &mut Materialization,
        &TestDocument,
        &mut Traced,
        usize,
    ) -> Result<Step, DriveError<&'static str>> = Materialization::step::<ChunkCodec, Traced>;
    assert_eq!(
        step(&mut driver, &doc, &mut state, 1).unwrap(),
        Step::Current
    );
    assert_eq!(state.trace, ["deliver", "payload", "payload", "checkpoint"]);
    assert_eq!(state.saved.as_ref(), Some(driver.checkpoint()));
    assert_eq!(
        step(&mut driver, &doc, &mut state, 1).unwrap(),
        Step::Current
    );
    assert_eq!(state.trace.len(), 4);
}

#[test]
fn a_followup_apply_failure_keeps_mutated_state_but_restores_the_saved_checkpoint() {
    struct FailFollowup {
        hash: CommitHash,
        calls: usize,
    }
    impl Materializer for FailFollowup {
        type Error = &'static str;
        fn apply(&mut self, _: &History, _: Vec<Event<Bytes>>) -> Result<Applied, Self::Error> {
            self.calls += 1;
            if self.calls == 1 {
                let mut report = Applied::default();
                report.wanted.push(self.hash);
                Ok(report)
            } else {
                Err("followup failed after mutation")
            }
        }
        fn checkpoint(&mut self, _: &Checkpoint) -> Result<(), Self::Error> {
            panic!("failed round must not checkpoint")
        }
    }
    let mut doc = document();
    let hash = add(&mut doc, b"value");
    let mut driver = Materialization::new(Follow::Working, true);
    let saved = driver.checkpoint().clone();
    let mut state = FailFollowup { hash, calls: 0 };
    assert_eq!(
        driver.step(&doc, &mut state, 1),
        Err(DriveError::Apply("followup failed after mutation"))
    );
    assert_eq!(state.calls, 2);
    assert_eq!(driver.checkpoint(), &saved);
    assert_eq!(
        driver.step(&doc, &mut state, 1),
        Err(DriveError::Stockfoil(Error::Poisoned))
    );
    assert_eq!(state.calls, 2);
}

#[test]
fn signed_views_deliver_drafts_inactive_then_activate_after_signing() {
    let signer = key(1);
    let mut doc = document();
    let hash = add(&mut doc, b"draft");
    let mut m = Materialization::new(Follow::Signed, true);
    let mut state = State::default();
    m.update(&doc, &mut state).unwrap();
    assert!(state.known.contains_key(&hash));
    assert!(state.active.is_empty());
    doc.sign_drafts(&signer).unwrap();
    let progress = m.update(&doc, &mut state).unwrap();
    assert_eq!(progress.activated, 1);
    assert_eq!(state.active, BTreeSet::from([hash]));
}

#[test]
fn malformed_reports_preserve_tokens_and_valid_payload_requests_are_deduplicated() {
    let mut doc = document();
    let first = add(&mut doc, b"first");
    let second = add(&mut doc, b"second");
    let unknown = CommitHash::from([0xff; 32]);
    assert!(!doc.ledger().contains(&unknown));
    let mut m = Materialization::new(Follow::Working, true);
    let saved = m.checkpoint().clone();
    let batch = m.plan(doc.ledger(), 1).unwrap().unwrap();
    let duplicate = Applied {
        rejected: vec![
            Rejection {
                hash: first,
                reason: "direct".into(),
                cascade: false,
            },
            Rejection {
                hash: first,
                reason: "cascade".into(),
                cascade: true,
            },
        ],
        wanted: vec![unknown],
    };
    // Rejection validation precedes payload lookup, and compares hashes rather than full records.
    assert!(matches!(
        m.complete(doc.ledger(), batch.clone(), duplicate),
        Err(Error::Protocol(_))
    ));
    assert_eq!(m.checkpoint(), &saved);
    assert!(matches!(
        m.complete(
            doc.ledger(),
            batch.clone(),
            Applied::default().reject(second, "outside source batch")
        ),
        Err(Error::Protocol(_))
    ));
    assert_eq!(
        m.complete(
            doc.ledger(),
            batch.clone(),
            Applied::default().want(unknown)
        )
        .unwrap_err(),
        Error::UnknownCommit(unknown)
    );
    assert_eq!(m.checkpoint(), &saved);
    let mut other = Materialization::new(Follow::Working, true);
    let foreign = other.plan(doc.ledger(), 1).unwrap().unwrap();
    assert_eq!(
        m.complete(doc.ledger(), foreign, Applied::default().want(unknown))
            .unwrap_err(),
        Error::StaleBatch
    );
    assert_eq!(m.checkpoint(), &saved);
    // Known payloads outside this source batch are permitted; repeats are normalized.
    let follow_up = m
        .complete(
            doc.ledger(),
            batch.clone(),
            Applied::default().want(second).want(first).want(second),
        )
        .unwrap()
        .unwrap();
    let expected: Vec<_> = BTreeSet::from([first, second])
        .into_iter()
        .map(|hash| Event::Payload {
            hash,
            payload: doc.ledger().payload_id(&hash).unwrap(),
        })
        .collect();
    assert_eq!(follow_up.events(), expected);
    assert_eq!(m.checkpoint(), &saved);
    assert_eq!(
        m.complete(doc.ledger(), batch, Applied::default())
            .unwrap_err(),
        Error::StaleBatch
    );
    assert!(
        m.complete(doc.ledger(), follow_up, Applied::default())
            .unwrap()
            .is_none()
    );
    assert!(m.checkpoint().is_partial());
}

#[test]
fn follow_up_report_can_upgrade_an_original_delivery_to_a_cascade() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let child = add(&mut doc, b"child");
    let mut m = Materialization::new(Follow::Working, true);
    let saved = m.checkpoint().clone();
    let batch = m.plan(doc.ledger(), 10).unwrap().unwrap();
    let payload = m
        .complete(
            doc.ledger(),
            batch,
            Applied::default().reject(root, "direct").want(child),
        )
        .unwrap()
        .unwrap();
    assert!(matches!(payload.events(), [Event::Payload { hash, .. }] if *hash == child));
    let upgrade = Applied {
        rejected: vec![Rejection {
            hash: root,
            reason: "now cascade".into(),
            cascade: true,
        }],
        wanted: vec![],
    };
    let correction = m.complete(doc.ledger(), payload, upgrade).unwrap().unwrap();
    assert_eq!(correction.events(), &[Event::Retract { hash: child }]);
    assert_eq!(m.checkpoint(), &saved);
    assert!(
        m.complete(doc.ledger(), correction, Applied::default())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        m.checkpoint().rejected().collect::<Vec<_>>(),
        vec![(root, true)]
    );
    assert!(m.is_current(doc.ledger()));
}

#[test]
fn corrections_precede_payloads_and_do_not_repeat_or_retract_direct_rejections() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let child = add(&mut doc, b"child");
    let descendant = add(&mut doc, b"descendant");
    let mut m = Materialization::new(Follow::Working, true);
    let saved = m.checkpoint().clone();
    let first = m.plan(doc.ledger(), 100).unwrap().unwrap();
    let report = Applied {
        rejected: vec![
            Rejection {
                hash: root,
                reason: "cascade root".into(),
                cascade: true,
            },
            Rejection {
                hash: child,
                reason: "already rejected by application".into(),
                cascade: false,
            },
        ],
        wanted: vec![child, root, child],
    };
    let correction = m.complete(doc.ledger(), first, report).unwrap().unwrap();
    let mut expected = vec![Event::Retract { hash: descendant }];
    expected.extend(
        BTreeSet::from([child, root])
            .into_iter()
            .map(|hash| Event::Payload {
                hash,
                payload: doc.ledger().payload_id(&hash).unwrap(),
            }),
    );
    assert_eq!(correction.events(), expected);
    assert_eq!(m.checkpoint(), &saved);
    let payload = m
        .complete(
            doc.ledger(),
            correction,
            Applied::default().want(descendant),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        payload.events(),
        &[Event::Payload {
            hash: descendant,
            payload: doc.ledger().payload_id(&descendant).unwrap()
        }]
    );
    assert_eq!(m.checkpoint(), &saved);
    assert!(
        m.complete(doc.ledger(), payload, Applied::default())
            .unwrap()
            .is_none()
    );
    assert!(m.is_current(doc.ledger()));
    assert_eq!(
        m.checkpoint().rejected().collect::<BTreeMap<_, _>>(),
        BTreeMap::from([(root, true), (child, false)])
    );
}

#[test]
fn reset_correction_suppresses_payloads_and_replays_the_original_capture_after_growth() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let child = add(&mut doc, b"child");
    let mut m = Materialization::new(Follow::Working, false);
    let saved = m.checkpoint().clone();
    let first = m.plan(doc.ledger(), 100).unwrap().unwrap();
    let later = add(&mut doc, b"arrived during feedback");
    let report = Applied {
        rejected: vec![Rejection {
            hash: root,
            reason: "cascade".into(),
            cascade: true,
        }],
        wanted: vec![child, later],
    };
    let reset = m.complete(doc.ledger(), first, report).unwrap().unwrap();
    assert_eq!(reset.events(), &[Event::Reset]);
    assert_eq!(m.checkpoint(), &saved);
    assert!(
        m.complete(doc.ledger(), reset, Applied::default())
            .unwrap()
            .is_none()
    );
    assert!(m.checkpoint().is_partial());
    assert_eq!(
        m.checkpoint().rejected().collect::<Vec<_>>(),
        vec![(root, true)]
    );
    let replay = m.plan(doc.ledger(), 100).unwrap().unwrap();
    assert!(!replay.is_rebuild());
    assert_eq!(
        replay
            .events()
            .iter()
            .map(|event| match event {
                Event::Deliver {
                    hash,
                    active: false,
                    ..
                } => *hash,
                other => panic!("expected inactive captured delivery, got {other:?}"),
            })
            .collect::<Vec<_>>(),
        vec![root, child]
    );
    assert!(
        m.complete(doc.ledger(), replay, Applied::default())
            .unwrap()
            .is_none()
    );
    let fresh = m.plan(doc.ledger(), 100).unwrap().unwrap();
    assert!(
        matches!(fresh.events(), [Event::Deliver { hash, active: false, .. }] if *hash == later)
    );
    assert!(
        m.complete(doc.ledger(), fresh, Applied::default())
            .unwrap()
            .is_none()
    );
    assert!(m.is_current(doc.ledger()));
}

#[test]
fn missing_pin_does_not_bind_and_foreign_completion_retains_pending_work() {
    let first_history = document();
    let mut correct = document();
    add(&mut correct, b"ready");
    let unknown = CommitHash::from([255; 32]);
    let mut m = Materialization::new(Follow::Pinned(vec![unknown]), true);
    let saved = m.checkpoint().clone();
    assert_eq!(
        m.plan(first_history.ledger(), 0).unwrap_err(),
        Error::UnknownCommit(unknown)
    );
    assert_eq!(m.checkpoint(), &saved);
    assert!(!m.is_dirty());
    m.set_follow(Follow::Working);
    // A later budget error keeps the binding established by the successful guard.
    assert!(matches!(m.plan(correct.ledger(), 0), Err(Error::Limit(_))));
    assert_eq!(
        m.plan(first_history.ledger(), 1).unwrap_err(),
        Error::WrongHistory
    );
    let batch = m.plan(correct.ledger(), 100).unwrap().unwrap();
    let saved = m.checkpoint().clone();
    assert_eq!(
        m.complete(first_history.ledger(), batch.clone(), Applied::default())
            .unwrap_err(),
        Error::WrongHistory
    );
    assert_eq!(m.checkpoint(), &saved);
    assert!(
        m.complete(correct.ledger(), batch, Applied::default())
            .unwrap()
            .is_none()
    );
    assert!(m.is_current(correct.ledger()));
}

#[test]
fn restored_checkpoint_can_retry_binding_and_rejects_an_identical_foreign_graph() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let mut identical = document();
    assert_eq!(add(&mut identical, b"root"), root);
    let mut m = Materialization::new(Follow::Working, true);
    let batch = m.plan(doc.ledger(), 100).unwrap().unwrap();
    assert!(
        m.complete(
            doc.ledger(),
            batch,
            Applied::default().reject(root, "invalid payload")
        )
        .unwrap()
        .is_none()
    );
    m.set_follow(Follow::Pinned(vec![root]));
    let checkpoint = Checkpoint::from_bytes(&m.checkpoint().to_bytes()).unwrap();
    let mut restored = Materialization::resume(checkpoint.clone(), true);
    let empty = document();
    assert_eq!(
        restored.plan(empty.ledger(), 1).unwrap_err(),
        Error::UnknownCommit(root)
    );
    assert_eq!(restored.checkpoint(), &checkpoint);
    assert!(restored.plan(doc.ledger(), 1).unwrap().is_none());
    assert!(restored.is_current(doc.ledger()));
    assert_eq!(restored.checkpoint(), &checkpoint);
    assert_eq!(
        restored.plan(identical.ledger(), 1).unwrap_err(),
        Error::WrongHistory
    );
    assert_eq!(restored.checkpoint(), &checkpoint);
}

#[test]
fn poisoned_guard_precedes_budget_and_foreign_history_errors() {
    let mut doc = document();
    add(&mut doc, b"root");
    let mut m = Materialization::new(Follow::Working, true);
    let saved = m.checkpoint().clone();
    let mut state = State {
        fail_checkpoint: true,
        ..State::default()
    };
    assert!(matches!(
        m.step(&doc, &mut state, 1),
        Err(DriveError::Checkpoint(_))
    ));
    let foreign = document();
    assert_eq!(m.plan(foreign.ledger(), 0).unwrap_err(), Error::Poisoned);
    assert_eq!(m.checkpoint(), &saved);
    assert_eq!(m.plan(doc.ledger(), 1).unwrap_err(), Error::Poisoned);
}

#[test]
fn new_deliveries_do_not_reset_but_activating_old_deliveries_does() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let mut m = Materialization::new(Follow::Working, false);
    let mut state = State::default();
    m.update(&doc, &mut state).unwrap();
    let child = add(&mut doc, b"child");
    m.update(&doc, &mut state).unwrap();
    assert_eq!(state.resets, 0);
    assert_eq!(state.active, BTreeSet::from([root, child]));
    doc.set_permissions(WritePermissions::new());
    m.update(&doc, &mut state).unwrap();
    assert_eq!(state.resets, 1);
    assert_eq!(state.known.len(), 2);
    assert!(state.active.is_empty());
    doc.set_permissions(WritePermissions::new().with_unlisted(WritePermission::Unbounded));
    // A budget of one is consumed by Reset before any captured record is replayed.
    assert_eq!(m.step(&doc, &mut state, 1).unwrap(), Step::More);
    assert_eq!(state.resets, 2);
    assert!(state.known.is_empty());
    assert!(m.checkpoint().is_partial());
    m.update(&doc, &mut state).unwrap();
    assert_eq!(state.resets, 2);
    assert_eq!(state.active, BTreeSet::from([root, child]));
}

#[test]
fn resumed_source_uses_saved_policy_and_frontier_before_applying_new_follow() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let child = add(&mut doc, b"child");
    let mut m = Materialization::new(Follow::Working, false);
    let first = m.plan(doc.ledger(), 1).unwrap().unwrap();
    assert!(matches!(first.events(), [Event::Deliver { hash, active: true, .. }] if *hash == root));
    m.complete(doc.ledger(), first, Applied::default()).unwrap();
    assert!(m.checkpoint().is_partial());
    m.set_follow(Follow::Signed);
    let saved = Checkpoint::from_bytes(&m.checkpoint().to_bytes()).unwrap();
    let later = add(&mut doc, b"later");
    doc.set_permissions(WritePermissions::new());
    let mut resumed = Materialization::resume(saved, false);
    let rest = resumed.plan(doc.ledger(), 100).unwrap().unwrap();
    assert!(matches!(rest.events(), [Event::Deliver { hash, active: true, .. }] if *hash == child));
    assert!(!rest.is_rebuild());
    resumed
        .complete(doc.ledger(), rest, Applied::default())
        .unwrap();
    assert_eq!(resumed.heads(), &[child]);
    assert_eq!(resumed.follow(), &Follow::Signed);
    let reset = resumed.plan(doc.ledger(), 1).unwrap().unwrap();
    assert_eq!(reset.events(), &[Event::Reset]);
    resumed
        .complete(doc.ledger(), reset, Applied::default())
        .unwrap();
    let replay = resumed.plan(doc.ledger(), 100).unwrap().unwrap();
    let deliveries: Vec<_> = replay
        .events()
        .iter()
        .map(|event| match event {
            Event::Deliver {
                hash,
                active: false,
                ..
            } => *hash,
            other => panic!("expected inactive delivery, got {other:?}"),
        })
        .collect();
    assert_eq!(deliveries, vec![root, child, later]);
    resumed
        .complete(doc.ledger(), replay, Applied::default())
        .unwrap();
    assert!(resumed.is_current(doc.ledger()));
}

#[test]
fn follow_target_selects_working_signed_or_exact_pins_and_copies_policy() {
    let signer = key(1);
    let mut doc = document_for(&signer);
    let root = signed(&mut doc, &signer, vec![], b"signed root");
    let child = add(&mut doc, b"draft child");
    let working = doc.ledger().view(&Follow::Working);
    let signed_view = doc.ledger().view(&Follow::Signed);
    let pin = vec![child, root, child];
    let pinned = doc.ledger().view(&Follow::Pinned(pin.clone()));
    let original_policy = doc.ledger().permissions().clone();
    assert_eq!(working.heads(), &[child]);
    assert_eq!(signed_view.heads(), &[root]);
    assert_eq!(pinned.heads(), pin);
    doc.set_permissions(WritePermissions::new());
    assert_eq!(working.write_permissions(), &original_policy);
    assert_eq!(signed_view.write_permissions(), &original_policy);
    assert_eq!(pinned.write_permissions(), &original_policy);
    assert_eq!(
        doc.ledger().view(&Follow::Working).write_permissions(),
        doc.ledger().permissions()
    );
}

#[test]
fn policy_structure_changes_require_an_empty_checkpoint_batch() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let policy = WritePermissions::new().with_unlisted(WritePermission::Unbounded);
    doc.set_permissions(policy.clone());
    let mut m = Materialization::new(Follow::Working, true);
    let mut state = State::default();
    m.update(&doc, &mut state).unwrap();
    assert!(m.is_current(doc.ledger()));
    let saved = m.checkpoint().clone();
    let mut explicit = policy;
    explicit.insert(doc.ledger().author(), WritePermission::Unbounded);
    doc.set_permissions(explicit);
    assert!(!m.is_current(doc.ledger()));
    let batch = m.plan(doc.ledger(), 1).unwrap().unwrap();
    assert!(batch.is_empty());
    assert_eq!(m.checkpoint(), &saved);
    assert!(!m.is_current(doc.ledger()));
    assert!(
        m.complete(doc.ledger(), batch, Applied::default())
            .unwrap()
            .is_none()
    );
    assert!(m.is_current(doc.ledger()));
    assert_eq!(state.active, BTreeSet::from([root]));
}

#[test]
fn current_requires_exact_pin_order_and_multiplicity() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let child = add(&mut doc, b"child");
    let mut m = Materialization::new(Follow::Pinned(vec![root, child]), true);
    m.update(&doc, &mut State::default()).unwrap();
    assert!(m.is_current(doc.ledger()));
    for pin in [vec![child, root], vec![child, root, root]] {
        m.set_follow(Follow::Pinned(pin.clone()));
        assert!(!m.is_current(doc.ledger()));
        let saved = m.checkpoint().clone();
        let batch = m.plan(doc.ledger(), 1).unwrap().unwrap();
        assert!(batch.is_empty());
        assert_eq!(m.checkpoint(), &saved);
        m.complete(doc.ledger(), batch, Applied::default()).unwrap();
        assert_eq!(m.heads(), pin);
        assert!(m.is_current(doc.ledger()));
    }
}

#[test]
fn checked_equality_preserves_policy_entries_and_frontier_sequences() {
    let a = key(1).author();
    let b = key(2).author();
    let x = CommitHash::from([1; 32]);
    let y = CommitHash::from([2; 32]);
    let first = WritePermissions::from([
        (a, WritePermission::Bounded(vec![x, y])),
        (b, WritePermission::Denied),
    ]);
    let second = WritePermissions::from([
        (b, WritePermission::Denied),
        (a, WritePermission::Bounded(vec![x, y])),
    ]);
    assert_eq!(first, second);
    assert_eq!(first, first.clone());
    let mut changed = second.clone();
    changed.remove(&b);
    assert_ne!(first, changed);
    changed = second.clone();
    changed.set_unlisted(WritePermission::Unbounded);
    assert_ne!(first, changed);
    changed = second.clone();
    changed.insert(a, WritePermission::Bounded(vec![y, x]));
    assert_ne!(first, changed);
    changed.insert(a, WritePermission::Bounded(vec![x, y, y]));
    assert_ne!(first, changed);
    changed.insert(a, WritePermission::Bounded(vec![x, x]));
    assert_ne!(first, changed);
}

#[test]
fn planning_keeps_concrete_ledger_function_pointer_calls_compatible() {
    let plan: fn(&mut Materialization, &Ledger, usize) -> Result<Option<Batch>, Error> =
        Materialization::plan;
    let current: fn(&Materialization, &Ledger) -> bool = Materialization::is_current;
    let complete: fn(
        &mut Materialization,
        &Ledger,
        Batch,
        Applied,
    ) -> Result<Option<Batch>, Error> = Materialization::complete;
    let doc = document();
    let mut m = Materialization::new(Follow::Working, true);
    let batch = plan(&mut m, doc.ledger(), 1).unwrap().unwrap();
    complete(&mut m, doc.ledger(), batch, Applied::default()).unwrap();
    assert!(current(&m, doc.ledger()));
    assert!(plan(&mut m, doc.ledger(), 1).unwrap().is_none());
}

#[derive(Default)]
struct UpdateRecording {
    deliveries: Vec<usize>,
    payloads: usize,
    checkpoints: Vec<Checkpoint>,
    request: Option<CommitHash>,
    fail_after_checkpoint: bool,
}

impl Materializer for UpdateRecording {
    type Error = &'static str;

    fn apply(&mut self, _: &History, events: Vec<Event<Bytes>>) -> Result<Applied, Self::Error> {
        let delivered = events
            .iter()
            .filter(|event| matches!(event, Event::Deliver { .. }))
            .count();
        if delivered > 0 {
            // Deliberately mutate before failing, to exercise the driver's recovery boundary.
            self.deliveries.push(delivered);
        }
        self.payloads += events
            .iter()
            .filter(|event| matches!(event, Event::Payload { .. }))
            .count();
        if self.fail_after_checkpoint && !self.checkpoints.is_empty() {
            return Err("later apply failed");
        }
        let mut report = Applied::default();
        if let Some(hash) = self.request.take() {
            report = report.want(hash);
        }
        Ok(report)
    }

    fn checkpoint(&mut self, value: &Checkpoint) -> Result<(), Self::Error> {
        self.checkpoints.push(value.clone());
        Ok(())
    }
}

fn update_document() -> (TestDocument, CommitHash, CommitHash) {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let mut tip = root;
    for i in 1u32..1025 {
        tip = add(&mut doc, &i.to_le_bytes());
    }
    (doc, root, tip)
}

#[test]
fn update_counts_multiple_steps_and_followups_then_stops_when_current() {
    let (doc, root, tip) = update_document();
    let mut driver = Materialization::new(Follow::Working, true);
    let mut state = UpdateRecording {
        request: Some(root),
        ..Default::default()
    };
    let update: fn(
        &mut Materialization,
        &TestDocument,
        &mut UpdateRecording,
    ) -> Result<Progress, DriveError<&'static str>> =
        Materialization::update::<ChunkCodec, UpdateRecording>;
    let report = update(&mut driver, &doc, &mut state).unwrap();
    assert_eq!(state.deliveries, [1024, 1]);
    assert_eq!(state.payloads, 1);
    assert_eq!(
        state
            .checkpoints
            .iter()
            .map(Checkpoint::is_partial)
            .collect::<Vec<_>>(),
        [true, false]
    );
    assert_eq!(report.heads_before, []);
    assert_eq!(report.heads_after, [tip]);
    assert_eq!(
        (
            report.delivered,
            report.activated,
            report.retracted,
            report.rejected
        ),
        (1025, 0, 0, 0)
    );
    assert!(!report.rebuilt && report.changed());
    assert_eq!(driver.checkpoint(), state.checkpoints.last().unwrap());
    assert!(driver.is_current(doc.ledger()));

    let quiet = driver.update(&doc, &mut state).unwrap();
    assert!(!quiet.changed());
    assert_eq!(quiet.heads_before, [tip]);
    assert_eq!(quiet.heads_after, [tip]);
    assert_eq!(state.deliveries, [1024, 1]);
    assert_eq!(state.payloads, 1);
    assert_eq!(state.checkpoints.len(), 2);
}

#[test]
fn update_later_failure_preserves_the_prior_checkpoint_but_not_application_state() {
    let (doc, _, tip) = update_document();
    let mut driver = Materialization::new(Follow::Working, true);
    let mut state = UpdateRecording {
        fail_after_checkpoint: true,
        ..Default::default()
    };
    assert!(matches!(
        driver.update(&doc, &mut state),
        Err(DriveError::Apply("later apply failed"))
    ));
    assert_eq!(state.deliveries, [1024, 1]);
    assert_eq!(state.checkpoints.len(), 1);
    let saved = state.checkpoints[0].clone();
    assert!(saved.is_partial());
    assert_eq!(driver.checkpoint(), &saved);
    assert!(matches!(
        driver.update(&doc, &mut state),
        Err(DriveError::Stockfoil(Error::Poisoned))
    ));
    assert_eq!(state.deliveries, [1024, 1]);

    // Restore the application to the saved partial checkpoint before resuming.
    state.deliveries.pop();
    state.fail_after_checkpoint = false;
    let mut resumed = Materialization::resume(saved, true);
    let report = resumed.update(&doc, &mut state).unwrap();
    assert_eq!(report.delivered, 1);
    assert_eq!(report.heads_after, [tip]);
    assert_eq!(state.deliveries, [1024, 1]);
    assert!(resumed.is_current(doc.ledger()));
}

#[test]
fn mark_local_checks_exact_metadata_before_normalizing_parent_heads() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let mut driver = Materialization::new(Follow::Working, true);
    driver.update(&doc, &mut State::default()).unwrap();
    let saved = driver.checkpoint().clone();
    let draft = doc
        .commit(b"local".as_slice(), Some(vec![root, root]))
        .unwrap();
    let mark: fn(&mut Materialization, &Ledger, &Draft) -> Result<(), Error> =
        Materialization::mark_local;
    let mut wrong_parents = draft.clone();
    wrong_parents.parents.pop();
    let mut wrong_author = draft.clone();
    wrong_author.author = author(&key(99));
    let mut missing = draft.clone();
    missing.hash = CommitHash::from([255; 32]);
    for invalid in [wrong_parents, wrong_author, missing] {
        assert!(matches!(
            mark(&mut driver, doc.ledger(), &invalid),
            Err(Error::Protocol("draft metadata does not match the ledger"))
        ));
        assert_eq!(driver.checkpoint(), &saved);
        assert!(!driver.is_dirty());
    }
    // Exact metadata includes the repeated parent; head comparison ignores repeats.
    mark(&mut driver, doc.ledger(), &draft).unwrap();
    assert!(driver.is_current(doc.ledger()) && driver.is_dirty());
    let acknowledged = driver.checkpoint().clone();
    assert!(matches!(
        driver.mark_local(doc.ledger(), &draft),
        Err(Error::Stale)
    ));
    assert_eq!(driver.checkpoint(), &acknowledged);
    assert!(driver.is_dirty());
}

#[test]
fn mark_local_moves_the_pin_and_update_checkpoints_without_echoing() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let mut driver = Materialization::new(Follow::Pinned(vec![root, root]), true);
    let mut state = UpdateRecording::default();
    driver.update(&doc, &mut state).unwrap();
    let draft = doc
        .commit(b"already applied".as_slice(), Some(vec![root, root]))
        .unwrap();
    // Model the application's local edit, which precedes acknowledgement.
    state.deliveries.push(1);
    driver.mark_local(doc.ledger(), &draft).unwrap();
    assert_eq!(driver.follow(), &Follow::Pinned(vec![draft.hash]));
    assert_eq!(driver.heads(), [draft.hash]);
    assert!(driver.is_dirty() && driver.is_current(doc.ledger()));
    let report = driver.update(&doc, &mut state).unwrap();
    assert!(!report.changed());
    assert_eq!(state.deliveries, [1, 1]);
    assert_eq!(state.checkpoints.len(), 2);
    assert_eq!(state.checkpoints.last().unwrap(), driver.checkpoint());
    assert!(!driver.is_dirty());
}

#[test]
fn mark_local_rejects_intervening_deliveries_and_changed_policy() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let mut driver = Materialization::new(Follow::Working, true);
    driver.update(&doc, &mut State::default()).unwrap();
    let saved = driver.checkpoint().clone();
    let permissions = doc.permissions().clone();
    let draft = doc.commit(b"local".as_slice(), Some(vec![root])).unwrap();
    doc.set_permissions(WritePermissions::default());
    assert!(matches!(
        driver.mark_local(doc.ledger(), &draft),
        Err(Error::Stale)
    ));
    assert_eq!(driver.checkpoint(), &saved);
    doc.set_permissions(permissions);
    add(&mut doc, b"intervening");
    assert!(matches!(
        driver.mark_local(doc.ledger(), &draft),
        Err(Error::Stale)
    ));
    assert_eq!(driver.checkpoint(), &saved);
    assert!(!driver.is_dirty());
    // A failed acknowledgement does not poison ordinary driving.
    let report = driver.update(&doc, &mut State::default()).unwrap();
    assert_eq!(report.delivered, 2);
}

#[test]
fn mark_local_rejects_denied_drafts_and_signed_views() {
    let mut doc = document();
    doc.set_permissions(WritePermissions::default());
    let mut driver = Materialization::new(Follow::Working, true);
    driver.update(&doc, &mut State::default()).unwrap();
    let saved = driver.checkpoint().clone();
    let draft = doc.commit(b"denied".as_slice(), Some(vec![])).unwrap();
    assert!(matches!(
        driver.mark_local(doc.ledger(), &draft),
        Err(Error::Protocol("local draft does not participate"))
    ));
    assert_eq!(driver.checkpoint(), &saved);
    driver.set_follow(Follow::Signed);
    let signed = driver.checkpoint().clone();
    assert!(matches!(
        driver.mark_local(doc.ledger(), &draft),
        Err(Error::Protocol("cannot edit a signed-only view"))
    ));
    assert_eq!(driver.checkpoint(), &signed);
}

#[test]
fn mark_local_keeps_an_outstanding_batch_and_partial_checkpoint() {
    let mut doc = document();
    add(&mut doc, b"a");
    let draft = doc.commit(b"b".as_slice(), None).unwrap();
    let mut driver = Materialization::new(Follow::Working, true);
    let pending = driver.plan(doc.ledger(), 1).unwrap().unwrap();
    let saved = driver.checkpoint().clone();
    assert!(matches!(
        driver.mark_local(doc.ledger(), &draft),
        Err(Error::Stale)
    ));
    assert_eq!(driver.checkpoint(), &saved);
    driver
        .complete(doc.ledger(), pending, Applied::default())
        .unwrap();
    let partial = driver.checkpoint().clone();
    assert!(partial.is_partial());
    assert!(matches!(
        driver.mark_local(doc.ledger(), &draft),
        Err(Error::Stale)
    ));
    assert_eq!(driver.checkpoint(), &partial);
}

#[test]
fn local_queries_preserve_metadata_and_ignore_policy_for_causal_difference() {
    let mut doc = document();
    let root = add(&mut doc, b"root");
    let left = doc.commit(b"left".as_slice(), Some(vec![root])).unwrap();
    let right = doc.commit(b"right".as_slice(), Some(vec![root])).unwrap();
    let join = doc
        .commit(
            b"join".as_slice(),
            Some(vec![right.hash, left.hash, right.hash]),
        )
        .unwrap();
    doc.set_permissions(WritePermissions::default());
    assert_eq!(doc.ledger().parents(&join.hash), Some(join.parents.clone()));
    assert_eq!(doc.ledger().author_of(&join.hash), Some(join.author));
    let missing = CommitHash::from([255; 32]);
    assert_eq!(doc.ledger().parents(&missing), None);
    assert_eq!(doc.ledger().author_of(&missing), None);
    let mut ready = [left.hash, right.hash];
    ready.sort();
    let iter = doc.ledger().commits_since(&[root, root]).unwrap();
    assert_eq!(iter.len(), 3);
    assert_eq!(iter.collect::<Vec<_>>(), [ready[0], ready[1], join.hash]);
    assert_eq!(
        doc.history().commits_since(&[join.hash]).unwrap().count(),
        0
    );
    assert!(
        matches!(doc.ledger().commits_since(&[missing]), Err(Error::UnknownCommit(h)) if h == missing)
    );
    let capture = doc.ledger().ancestry(&[join.hash]).unwrap();
    assert!(!capture.participates(doc.history(), &join.hash).unwrap());
}

#[test]
fn edit_none_runs_once_preserves_mutation_and_does_not_publish() {
    let mut doc = document();
    let mut m = Materialization::new(Follow::Working, false);
    let mut state = Counter::default();
    m.update(&doc, &mut state).unwrap();
    let checkpoint = m.checkpoint().clone();
    let epoch = doc.history().epoch();
    let mut calls = 0;
    let result = m
        .change(&mut doc, &mut state, |state| {
            calls += 1;
            state.value += 5;
            Ok::<_, &'static str>(None)
        })
        .unwrap();
    assert!(result.is_none());
    assert_eq!(calls, 1);
    assert_eq!(state.value, 5);
    assert_eq!(doc.history().epoch(), epoch);
    assert_eq!(m.checkpoint(), &checkpoint);
    assert!(!m.is_dirty());
    assert!(!m.update(&doc, &mut state).unwrap().changed());
    assert_eq!(state.value, 5);
}

#[test]
fn edit_error_keeps_exact_error_and_state_mutation_then_poison_blocks_retry() {
    let mut doc = document();
    let mut m = Materialization::new(Follow::Working, false);
    let mut state = Counter::default();
    m.update(&doc, &mut state).unwrap();
    let checkpoint = m.checkpoint().clone();
    let epoch = doc.history().epoch();
    let mut calls = 0;
    let result = m.change(&mut doc, &mut state, |state| {
        calls += 1;
        state.value = 17;
        Err::<Option<Vec<u8>>, _>("partially edited")
    });
    assert_eq!(result.unwrap_err(), DriveError::Edit("partially edited"));
    assert_eq!(calls, 1);
    assert_eq!(state.value, 17);
    assert_eq!(doc.history().epoch(), epoch);
    assert_eq!(m.checkpoint(), &checkpoint);
    let retry = m.change(&mut doc, &mut state, |_| -> Result<_, &'static str> {
        panic!("a poisoned edit must not call its closure")
    });
    assert_eq!(retry.unwrap_err(), DriveError::Stockfoil(Error::Poisoned));
    assert_eq!(state.value, 17);
}

#[test]
fn duplicate_publication_after_edit_fails_acknowledgement_without_rolling_back_state() {
    let mut doc = document();
    let bytes = 3u64.to_le_bytes();
    let root = doc.commit(bytes.as_slice(), Some(vec![])).unwrap();
    let mut m = Materialization::new(Follow::Pinned(vec![]), false);
    let mut state = Counter::default();
    m.update(&doc, &mut state).unwrap();
    let checkpoint = m.checkpoint().clone();
    let epoch = doc.history().epoch();
    let result = m.change(&mut doc, &mut state, |state| {
        state.value = 3;
        Ok::<_, &'static str>(Some(bytes.to_vec()))
    });
    assert_eq!(result.unwrap_err(), DriveError::Stockfoil(Error::Stale));
    assert_eq!(state.value, 3);
    assert_eq!(doc.history().epoch(), epoch);
    assert_eq!(doc.payload(&root.hash).unwrap().as_ref(), bytes);
    assert_eq!(m.checkpoint(), &checkpoint);
    assert_eq!(
        m.update(&doc, &mut state).unwrap_err(),
        DriveError::Stockfoil(Error::Poisoned)
    );
}

#[test]
fn stale_edit_precedes_signed_view_rejection_without_calling_the_closure() {
    let mut doc = document();
    let mut m = Materialization::new(Follow::Signed, false);
    let mut state = Counter::default();
    m.update(&doc, &mut state).unwrap();
    add(&mut doc, &1u64.to_le_bytes());
    let result = m.change(&mut doc, &mut state, |_| -> Result<_, &'static str> {
        panic!("a stale edit must not run")
    });
    assert_eq!(result.unwrap_err(), DriveError::Stockfoil(Error::Stale));
    m.update(&doc, &mut state).unwrap();
    assert_eq!(state.value, 0);
}
