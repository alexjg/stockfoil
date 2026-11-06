//! The planner protocol, independent states, resumability and failure handling.
mod support;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    doc.set_permissions(HashMap::from([
        (author(&alice), WritePermission::Unbounded),
        (author(&bob), WritePermission::Unbounded),
    ]));
    let p = live.update(&doc, &mut state).unwrap();
    assert_eq!(p.delivered, 0);
    assert_eq!(p.activated, 1);
    assert!(state.active.contains(&hidden));
    assert_eq!(historical.active, BTreeSet::from([root]));
    doc.set_permissions(HashMap::from([(
        author(&alice),
        WritePermission::Unbounded,
    )]));
    let p = live.update(&doc, &mut state).unwrap();
    assert_eq!(p.retracted, 1);
    assert_eq!(state.active, BTreeSet::from([root, descendant]));
    assert!(!live.update(&doc, &mut state).unwrap().changed());
}

#[test]
fn unsupported_visibility_changes_reset_then_redeliver_in_batches() {
    let mut doc = document();
    let first = add(&mut doc, b"a");
    add(&mut doc, b"b");
    let mut m = Materialization::new(Follow::Working, false);
    let mut state = State::default();
    m.update(&doc, &mut state).unwrap();
    doc.set_permissions(HashMap::from([(
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
            doc.set_permissions(HashMap::new());
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
        doc.set_permissions(HashMap::new());
        m.update(&doc, &mut state).unwrap();
        doc.set_permissions(HashMap::from([(doc.author(), WritePermission::Unbounded)]));
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
        rebuilt.set_permissions(HashMap::from([
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
