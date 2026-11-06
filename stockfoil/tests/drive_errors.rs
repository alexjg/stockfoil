//! Application errors retain their values and phase; recovery rules do not change.
mod support;

use std::error::Error as _;
use std::io;
use stockfoil::*;
use support::*;

// Deliberately not Clone. The boxed detail lets us check that no replacement
// error was manufactured from its display text, even when a replica lifts it
// into an operation with a different edit-error type.
#[derive(Debug)]
struct ApplicationError {
    detail: Box<str>,
    cause: io::Error,
}
impl std::fmt::Display for ApplicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}
impl std::error::Error for ApplicationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}
fn failure(detail: &str) -> (ApplicationError, *const u8) {
    let detail: Box<str> = detail.into();
    let address = detail.as_ptr();
    (
        ApplicationError {
            detail,
            cause: io::Error::from_raw_os_error(13),
        },
        address,
    )
}
fn check_application_source(error: &dyn std::error::Error, address: *const u8) {
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<ApplicationError>()
        .unwrap();
    assert_eq!(source.detail.as_ptr(), address);
    let cause = source
        .source()
        .unwrap()
        .downcast_ref::<io::Error>()
        .unwrap();
    assert_eq!(cause.raw_os_error(), Some(13));
}

#[derive(Default)]
struct State {
    known: Vec<CommitHash>,
    edits: usize,
    checkpoints: usize,
    fail_apply: Option<ApplicationError>,
    fail_checkpoint: Option<ApplicationError>,
}
impl Materializer for State {
    type Error = ApplicationError;
    fn apply(&mut self, _: &History, events: Vec<Event<Bytes>>) -> Result<Applied, Self::Error> {
        for event in events {
            match event {
                Event::Reset => self.known.clear(),
                Event::Deliver { hash, .. } => self.known.push(hash),
                _ => {}
            }
        }
        // Fail *after* mutation, to exercise poisoning rather than assuming
        // that application errors imply a rollback.
        if let Some(error) = self.fail_apply.take() {
            return Err(error);
        }
        Ok(Applied::default())
    }
    fn checkpoint(&mut self, _: &Checkpoint) -> Result<(), Self::Error> {
        self.checkpoints += 1;
        match self.fail_checkpoint.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    fn supports_visibility_changes(&self) -> bool {
        true
    }
}
fn replica(state: State) -> Replica<ChunkCodec, State> {
    Replica::new(ChunkCodec::default(), author(&key(1)), state)
}

#[test]
fn commit_keeps_the_original_apply_error_and_does_not_undo_the_commit() {
    let (failure, address) = failure("apply detail");
    let mut replica = replica(State {
        fail_apply: Some(failure),
        ..State::default()
    });
    let saved = replica.materialization().checkpoint().clone();
    let error = replica.commit(b"write").unwrap_err();
    check_application_source(&error, address);
    match error {
        DriveError::Apply(error) => {
            assert_eq!(&*error.detail, "apply detail");
            assert_eq!(error.detail.as_ptr(), address);
        }
        other => panic!("wrong phase: {other:?}"),
    }
    assert_eq!(replica.state().known.len(), 1); // application partially mutated
    assert_eq!(replica.document().history().len(), 1); // commit already exists
    assert!(replica.document().has_drafts());
    assert_eq!(replica.materialization().checkpoint(), &saved);
    assert!(matches!(
        replica.update(),
        Err(DriveError::Stockfoil(Error::Poisoned))
    ));

    replica.rebuild(State::default()).unwrap();
    assert_eq!(replica.state().known.len(), 1);
    assert!(!replica.update().unwrap().changed());
}

#[test]
fn checkpoint_errors_keep_their_source_and_last_saved_progress() {
    let mut doc = document();
    doc.commit(b"write".as_slice(), None).unwrap();
    let mut planner = Materialization::new(Follow::Working, true);
    let saved = planner.checkpoint().clone();
    let (failure, address) = failure("checkpoint detail");
    let mut state = State {
        fail_checkpoint: Some(failure),
        ..State::default()
    };
    let error = planner.step(&doc, &mut state, 1).unwrap_err();
    check_application_source(&error, address);
    assert!(matches!(error, DriveError::Checkpoint(_)));
    assert_eq!(state.known.len(), 1);
    assert_eq!(state.checkpoints, 1);
    assert_eq!(planner.checkpoint(), &saved);
    assert!(matches!(
        planner.update(&doc, &mut state),
        Err(DriveError::Stockfoil(Error::Poisoned))
    ));
    assert_eq!(state.checkpoints, 1); // no retry of potentially mutated state

    let mut restored = Materialization::resume(saved, true);
    let mut restored_state = State::default();
    restored.update(&doc, &mut restored_state).unwrap();
    assert_eq!(restored_state.known.len(), 1);
}

#[test]
fn edit_errors_have_an_independent_type_and_poison_without_creating_a_commit() {
    let mut replica = replica(State::default());
    replica.update().unwrap();
    let saved = replica.materialization().checkpoint().clone();
    // M = ApplicationError, E = io::Error: neither needs to be Clone, and
    // neither is coerced to the other's type.
    let error = replica
        .change_draft(|state| {
            state.edits += 1;
            Err::<Option<Vec<u8>>, _>(io::Error::from_raw_os_error(28))
        })
        .unwrap_err();
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(28)
    );
    match error {
        DriveError::Edit(error) => assert_eq!(error.raw_os_error(), Some(28)),
        other => panic!("wrong phase: {other:?}"),
    }
    assert_eq!(replica.state().edits, 1);
    assert!(replica.document().history().is_empty());
    assert_eq!(replica.materialization().checkpoint(), &saved);
    assert!(matches!(
        replica.update(),
        Err(DriveError::Stockfoil(Error::Poisoned))
    ));
}

#[test]
fn catching_up_before_an_edit_preserves_materializer_errors_with_the_edit_type() {
    let (failure, address) = failure("catch-up failed");
    let mut replica = replica(State {
        fail_apply: Some(failure),
        ..State::default()
    });
    replica
        .document_mut()
        .commit(b"remote".as_slice(), None)
        .unwrap();
    let error = replica
        .change_draft(|_| -> Result<Option<Vec<u8>>, io::Error> {
            panic!("edit must not run if catching up fails")
        })
        .unwrap_err();
    check_application_source(&error, address);
    assert!(matches!(error, DriveError::Apply(_)));
    assert!(matches!(
        replica.update(),
        Err(DriveError::Stockfoil(Error::Poisoned))
    ));
}

#[test]
fn signing_after_an_edit_preserves_checkpoint_errors_with_the_edit_type() {
    let (failure, address) = failure("saving signed edit failed");
    let mut replica = replica(State::default());
    let error = replica
        .change(&key(1), |state| {
            state.edits += 1;
            state.fail_checkpoint = Some(failure);
            Ok::<_, io::Error>(Some(b"local edit".to_vec()))
        })
        .unwrap_err();
    check_application_source(&error, address);
    assert!(matches!(error, DriveError::Checkpoint(_)));
    let hashes: Vec<_> = replica.document().history().traverse().collect();
    assert_eq!(hashes.len(), 1);
    assert!(replica.document().ledger().is_admitted(&hashes[0]));
    assert_eq!(replica.state().edits, 1);
    assert!(matches!(
        replica.update(),
        Err(DriveError::Stockfoil(Error::Poisoned))
    ));
}

#[test]
fn stockfoil_errors_keep_their_variant_and_source_without_poisoning_preflight() {
    let mut doc = document();
    let mut planner = Materialization::new(Follow::Working, true);
    let mut state = State::default();
    let error = planner.step(&doc, &mut state, 0).unwrap_err();
    assert!(matches!(error, DriveError::Stockfoil(Error::Limit(_))));
    assert!(matches!(
        error.source().unwrap().downcast_ref::<Error>(),
        Some(Error::Limit(_))
    ));
    planner.update(&doc, &mut state).unwrap();

    let error = planner.change(
        &mut doc,
        &mut state,
        |_| -> Result<Option<Vec<u8>>, io::Error> { Ok(None) },
    );
    assert!(error.unwrap().is_none());
    assert!(planner.is_current(doc.ledger()));
}

#[test]
fn string_errors_remain_matchable_and_displayable() {
    struct Strings;
    impl Materializer for Strings {
        type Error = &'static str;
        fn apply(&mut self, _: &History, _: Vec<Event<Bytes>>) -> Result<Applied, Self::Error> {
            Err("keep this exact error")
        }
    }
    let mut replica = Replica::new(ChunkCodec::default(), author(&key(1)), Strings);
    let error = replica.commit(b"write").unwrap_err();
    assert_eq!(error, DriveError::Apply("keep this exact error"));
    assert!(error.to_string().contains("keep this exact error"));
}
