//! A valid template item, received by a host whose replies are chosen by the
//! fuzz input: correct, stale ticket, wrong kind, short reads, failures,
//! garbage reads and decode results, early EOF, duplicates, deferrals, and
//! advancing with the wrong history.
//!
//! Invariants: no panic; the admission either admits exactly the item's
//! commits or fails (it never waits: dependencies are pre-admitted); `Stale`
//! and `WrongKind` replies change neither `counts()` nor the outstanding
//! request; a failed admission never changes the history.
#![no_main]
use arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;
use std::collections::HashMap;
use stockfoil::*;
use stockfoil_fuzz::codec::format;
use stockfoil_fuzz::host::*;
use stockfoil_fuzz::model::Dag;
use stockfoil_fuzz::templates::templates;

thread_local! {
    static CACHE: RefCell<HashMap<(usize, bool), (History, Host)>> = RefCell::new(HashMap::new());
}

const STATUSES: [DecodeStatus; 4] = [
    DecodeStatus::Working,
    DecodeStatus::NeedInput,
    DecodeStatus::EndMember,
    DecodeStatus::Finished,
];

/// Snapshot of what a rejected reply must not change.
fn snapshot(op: &Admission) -> (Counts, Option<(Ticket, String)>) {
    (op.counts(), op.request().map(|(t, r)| (t, format!("{r:?}"))))
}

fn run(u: &mut Unstructured) -> Result<()> {
    let t = templates();
    let index = u.choose_index(t.items.len())?;
    let only_item: bool = u.arbitrary()?;
    let item = &t.items[index];
    let (mut history, mut host) = CACHE
        .with(|c| c.borrow_mut().remove(&(index, only_item)))
        .unwrap_or_else(|| {
            let mut host = Host::default();
            let history = t.receiver_without(index, only_item, &mut host);
            (history, host)
        });
    for d in &item.deps {
        assert!(history.is_admitted(d), "harness: dependency not pre-admitted");
    }
    let blob = host.store(item.bytes.clone());
    let mut options = ReceiveOptions::new(format());
    options.read_size = [1, 2, 3, 7, 64, 100, 4096, 65536][u.choose_index(8)?];
    options.decode_capacity = [1, 2, 3, 7, 64, 100, 4096, 65536][u.choose_index(8)?];
    options.quantum = [1, 2, 64, 16384, 1 << 20][u.choose_index(5)?];
    let cfg = DriveConfig::default();
    let (epoch, len) = (history.epoch(), history.len());
    let before: Vec<Option<PayloadId>> =
        item.hashes.iter().map(|h| history.lookup(h).map(|i| i.payload)).collect();
    let mut op = history.receive(options);
    let mut st = OpState::default();
    let mut deferred = false;
    let mut steps = 0usize;
    let outcome: std::result::Result<Receipt, Error> = loop {
        steps += 1;
        assert!(steps < 1 << 22, "admission did not terminate");
        let (ticket, req) = match op.next() {
            Next::Continue => continue,
            Next::Done(r) => break Ok(r.clone()),
            Next::Failed(e) => break Err(e.clone()),
            Next::Waiting(h) => panic!("next() returned Waiting({h:?})"),
            Next::NeedHistory => {
                if !u.is_empty() && u.ratio(1, 4)? {
                    let mut other = History::new();
                    let snap = snapshot(&op);
                    assert!(matches!(other.advance(&mut op), Next::Failed(Error::WrongHistory)));
                    assert!(other.is_empty() && other.epoch() == 0, "wrong history mutated");
                    assert_eq!(snapshot(&op), snap, "wrong-history advance changed the op");
                }
                match history.advance(&mut op) {
                    Next::Done(r) => break Ok(r.clone()),
                    Next::Failed(e) => break Err(e.clone()),
                    Next::Waiting(h) => panic!("waiting for {h:?} although dependencies are admitted"),
                    Next::Request(t, r) => (t, Req::of(blob, &r)),
                    _ => continue,
                }
            }
            Next::Request(t, r) => (t, Req::of(blob, &r)),
            Next::AwaitReply => {
                assert!(deferred, "AwaitReply without a deferred request");
                let (t, r) = op.request().expect("AwaitReply without a request");
                (t, Req::of(blob, &r))
            }
        };
        deferred = false;
        // Exhausted input means honest replies from here on.
        let action = if u.is_empty() { 0 } else { u.int_in_range(0..=12)? };
        let snap = snapshot(&op);
        let rejected = |op: &Admission, r: std::result::Result<(), ReplyError>, want: ReplyError| {
            assert_eq!(r, Err(want), "bad reply was not rejected as expected");
            assert_eq!(snapshot(op), snap, "rejected reply changed the operation");
        };
        let owned = match action {
            1 => {
                // Stale ticket.
                let bad = Ticket::from_raw(ticket.get().wrapping_add(u.int_in_range(1..=3)?));
                let honest = host.execute(&req, &mut st.clone(), &cfg);
                let r = op.reply(bad, honest.reply());
                rejected(&op, r, ReplyError::Stale);
                deferred = true;
                continue;
            }
            2 => {
                let wrong = match req {
                    Req::Read { .. } => Owned::Bound(vec![1]),
                    Req::Decode { .. } => Owned::Read(vec![1], false),
                    Req::Bind { .. } => Owned::Decoded(0, vec![], DecodeStatus::Working),
                };
                let r = op.reply(ticket, wrong.reply());
                rejected(&op, r, ReplyError::WrongKind);
                deferred = true;
                continue;
            }
            3 => {
                // Defer: the request stays outstanding and nothing moves.
                assert!(matches!(history.advance(&mut op), Next::AwaitReply));
                assert_eq!(history.epoch(), epoch);
                assert_eq!(snapshot(&op), snap, "deferral changed the operation");
                deferred = true;
                continue;
            }
            4 => Owned::Failed("host failure".into()),
            5 => match &req {
                // Short but honest.
                Req::Read { .. } => {
                    let short = DriveConfig { read_limit: u.int_in_range(1..=8)?, ..cfg };
                    host.execute(&req, &mut st, &short)
                }
                Req::Decode { .. } => {
                    let short = DriveConfig { decode_limit: u.int_in_range(1..=8)?, ..cfg };
                    host.execute(&req, &mut st, &short)
                }
                Req::Bind { .. } => host.execute(&req, &mut st, &cfg),
            },
            6 => match &req {
                Req::Read { max, .. } => {
                    let n = u.int_in_range(0..=(*max).min(64))?;
                    Owned::Read(u.bytes(n)?.to_vec(), u.arbitrary()?)
                }
                Req::Decode { input, capacity, .. } => {
                    let consumed = u.int_in_range(0..=input.len() + 2)?;
                    let n = u.int_in_range(0..=(*capacity).min(64) + 1)?;
                    let out = u.bytes(n)?.to_vec();
                    Owned::Decoded(consumed, out, *u.choose(&STATUSES)?)
                }
                Req::Bind { members, .. } => {
                    let n = if u.arbitrary()? { *members } else { u.int_in_range(0..=members + 1)? };
                    Owned::Bound((0..n).map(|_| u.arbitrary()).collect::<Result<_>>()?)
                }
            },
            7 => match &req {
                // Early EOF: honest bytes cut short, flagged as the end.
                Req::Read { .. } => match host.execute(&req, &mut st, &cfg) {
                    Owned::Read(mut b, _) => {
                        let keep = u.int_in_range(0..=b.len())?;
                        b.truncate(keep);
                        Owned::Read(b, true)
                    }
                    other => other,
                },
                _ => host.execute(&req, &mut st, &cfg),
            },
            8 => match &req {
                Req::Read { .. } => Owned::Read(vec![], false),
                _ => host.execute(&req, &mut st, &cfg),
            },
            9 => match &req {
                // Honest decoder output with a wrong status.
                Req::Decode { .. } => match host.execute(&req, &mut st, &cfg) {
                    Owned::Decoded(c, o, _) => Owned::Decoded(c, o, *u.choose(&STATUSES)?),
                    other => other,
                },
                _ => host.execute(&req, &mut st, &cfg),
            },
            10 => {
                // Duplicate: answer, then answer again.
                let owned = host.execute(&req, &mut st, &cfg);
                let first = op.reply(ticket, owned.reply());
                if first.is_ok() {
                    if let Owned::Decoded(_, out, status) = &owned {
                        st.record(out, *status);
                    }
                }
                let snap = snapshot(&op);
                assert_eq!(op.reply(ticket, owned.reply()), Err(ReplyError::Stale), "duplicate reply accepted");
                assert_eq!(snapshot(&op), snap, "duplicate reply changed the operation");
                if let Err(ReplyError::Invalid(e)) = first {
                    assert!(matches!(op.next(), Next::Failed(f) if *f == e));
                }
                continue;
            }
            11 => match &req {
                // Read at the wrong offset (right length).
                Req::Read { item, offset, max } => {
                    let wrong = Req::Read { item: *item, offset: offset + u.int_in_range(1..=5)?, max: *max };
                    match host.execute(&wrong, &mut st, &cfg) {
                        Owned::Read(b, eof) if b.is_empty() => Owned::Read(vec![0], eof),
                        other => other,
                    }
                }
                _ => host.execute(&req, &mut st, &cfg),
            },
            _ => host.execute(&req, &mut st, &cfg),
        };
        match op.reply(ticket, owned.reply()) {
            Ok(()) => {
                if let Owned::Decoded(_, out, status) = &owned {
                    st.record(out, *status);
                }
            }
            Err(ReplyError::Invalid(e)) => {
                assert!(matches!(op.next(), Next::Failed(f) if *f == e), "Invalid reply did not fail the op");
                assert_eq!(op.reply(ticket, owned.reply()), Err(ReplyError::Stale));
            }
            Err(e) => panic!("reply to the outstanding request rejected: {e:?}"),
        }
    };
    match outcome {
        Ok(receipt) => {
            let got: Vec<_> = receipt.members.iter().map(|m| m.hash).collect();
            assert_eq!(got, item.hashes, "admitted different commits than the item's");
            assert_eq!(receipt.id, item.id);
            for (m, prior) in receipt.members.iter().zip(&before) {
                assert!(history.is_admitted(&m.hash));
                assert_eq!(m.newly_delivered, prior.is_none(), "newly_delivered wrong");
                if let Some(p) = prior {
                    assert_eq!(m.payload, *p, "a duplicate delivery changed the payload ID");
                }
            }
            let fresh = receipt.members.iter().filter(|m| m.newly_delivered).count();
            assert_eq!(history.len(), len + fresh);
            assert!(history.epoch() > epoch);
            Dag::of(&history).check(&history);
            // Changed history: rebuild next time.
        }
        Err(_) => {
            assert_eq!(history.epoch(), epoch, "failed admission changed the epoch");
            assert_eq!(history.len(), len, "failed admission delivered commits");
            for h in &item.hashes {
                let was = before[item.hashes.iter().position(|x| x == h).unwrap()].is_some();
                assert_eq!(history.lookup(h).is_some(), was);
            }
            assert!(matches!(op.next(), Next::Failed(_)));
            CACHE.with(|c| c.borrow_mut().insert((index, only_item), (history, host)));
        }
    }
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    let _ = run(&mut Unstructured::new(data));
});
