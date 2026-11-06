//! Planner operation sequences checked against the whole-history DAG model.
use crate::codec::*;
use crate::model::Dag;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use stockfoil::*;

#[derive(Clone, Default)]
struct State {
    known: BTreeMap<CommitHash, Bytes>,
    active: BTreeSet<CommitHash>,
}
impl Materializer for State {
    type Error = &'static str;
    fn apply(&mut self, _: &History, events: Vec<Event<Bytes>>) -> Result<Applied, Self::Error> {
        let mut result = Applied::default();
        for event in events {
            match event {
                Event::Reset => {
                    self.known.clear();
                    self.active.clear();
                }
                Event::Deliver {
                    hash,
                    payload,
                    active,
                    ..
                } => {
                    assert!(self.known.insert(hash, payload.clone()).is_none());
                    let byte = payload[0];
                    if byte % 11 == 0 {
                        result.rejected.push(Rejection {
                            hash,
                            reason: "test rule".into(),
                            cascade: byte % 22 == 0,
                        });
                    } else {
                        if active {
                            self.active.insert(hash);
                        }
                        if byte % 5 == 0 {
                            result.wanted.push(hash);
                        }
                    }
                }
                Event::Activate { hash } => {
                    assert!(self.known.contains_key(&hash));
                    self.active.insert(hash);
                }
                Event::Retract { hash } => {
                    self.active.remove(&hash);
                }
                Event::Payload { hash, payload } => {
                    assert_eq!(self.known.get(&hash), Some(&payload))
                }
            }
        }
        Ok(result)
    }
}
fn check(doc: &Document<ChunkCodec>, planner: &Materialization, state: &State) {
    if !planner.is_current(doc.ledger()) {
        return;
    }
    let dag = Dag::of(doc.history());
    assert_eq!(
        state.known.keys().copied().collect::<BTreeSet<_>>(),
        dag.hashes.iter().copied().collect()
    );
    let view = doc.ledger().view(planner.follow());
    let participating = dag.participation(&view);
    let rejected: HashMap<_, _> = planner.checkpoint().rejected().collect();
    let expected: BTreeSet<_> = participating
        .iter(dag.len())
        .filter(|&i| {
            !rejected.contains_key(&dag.hashes[i])
                && !rejected
                    .iter()
                    .any(|(root, cascade)| *cascade && dag.anc[i].contains(dag.index[root]))
        })
        .map(|i| dag.hashes[i])
        .collect();
    assert_eq!(
        state.active, expected,
        "planner state differs from whole-history model"
    );
}

/// Input chunks choose edits, signing, receiving another author, policy,
/// branches, bounded steps, save/resume and full updates. State/checkpoint are
/// always restored together; no process-local payload IDs are persisted.
pub fn exercise(data: &[u8]) {
    let mut doc = document();
    let supports = data.first().is_none_or(|b| b & 1 == 0);
    let mut planner = Materialization::new(Follow::Working, supports);
    let mut state = State::default();
    for (ordinal, bytes) in data.as_chunks::<4>().0.iter().take(64).enumerate() {
        let hashes: Vec<_> = doc.history().traverse().collect();
        let picked = hashes.get(bytes[1] as usize % hashes.len().max(1)).copied();
        match bytes[0] % 8 {
            0 => {
                let parents = if bytes[2] & 1 == 0 {
                    picked.into_iter().collect()
                } else {
                    doc.heads(Heads::Delivered)
                };
                doc.commit(vec![bytes[3], ordinal as u8], Some(parents))
                    .unwrap();
            }
            1 => {
                doc.sign_drafts(&key(1)).unwrap();
            }
            2 => {
                let permission = match bytes[2] % 3 {
                    0 => WritePermission::Denied,
                    1 => WritePermission::Unbounded,
                    _ => WritePermission::Bounded(picked.into_iter().collect()),
                };
                doc.set_permissions(HashMap::from([
                    (author(&key(1)), permission),
                    (
                        author(&key(2)),
                        if bytes[3] & 1 == 0 {
                            WritePermission::Unbounded
                        } else {
                            WritePermission::Denied
                        },
                    ),
                ]));
            }
            3 => planner.set_follow(match bytes[2] % 3 {
                0 => Follow::Working,
                1 => Follow::Signed,
                _ => Follow::Pinned(picked.into_iter().collect()),
            }),
            4 => {
                planner
                    .step(&doc, &mut state, 1 + bytes[2] as usize % 8)
                    .unwrap();
            }
            5 => {
                let checkpoint = Checkpoint::from_bytes(&planner.checkpoint().to_bytes()).unwrap();
                planner = Materialization::resume(checkpoint, supports);
                state = state.clone();
            }
            6 => {
                doc.sign_drafts(&key(1)).unwrap();
                let parents = doc.heads(Heads::Admitted);
                signed(&mut doc, &key(2), parents, &[bytes[3], ordinal as u8]);
            }
            _ => {
                planner.update(&doc, &mut state).unwrap();
            }
        }
        check(&doc, &planner, &state);
    }
    planner.update(&doc, &mut state).unwrap();
    check(&doc, &planner, &state);
}

#[cfg(test)]
mod tests {
    #[test]
    fn deterministic_model_sequences() {
        for seed in 1..=16u64 {
            let mut n = seed;
            let mut bytes = Vec::new();
            for _ in 0..192 {
                n ^= n << 13;
                n ^= n >> 7;
                n ^= n << 17;
                bytes.push(n as u8);
            }
            super::exercise(&bytes);
        }
    }
}
