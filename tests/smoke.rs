use std::{collections::{BTreeSet, HashMap}, ops::ControlFlow};

use eyre::{eyre, Context};
use stockfoil::{BlobHash, CommitContents, CommitGraph, CommitHash, CommitMeta};

enum Op {
    Add{
        value: String
    },
    Remove {
        predecessors: Vec<CommitHash>
    },
}

struct OrSetCommit {
    ops: Vec<Op>
}

impl OrSetCommit {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        bijoux::u64::encode(self.ops.len() as u64, &mut out);
        for op in &self.ops {
            match op {
                Op::Add { value } => {
                    out.push(1);
                    bijoux::u64::encode(value.as_bytes().len() as u64, &mut out);
                    out.extend_from_slice(value.as_bytes());
                },
                Op::Remove { predecessors } => {
                    out.push(2);
                    bijoux::u64::encode(predecessors.len() as u64, &mut out);
                    for pred in predecessors {
                        out.extend_from_slice(pred.as_slice());
                    }
                }
            }
        }
        out
    }

    fn decode(data: &[u8]) -> eyre::Result<Self> {
        let (mut num_ops, mut offset) = bijoux::u64::decode(data).wrap_err("failed to decode num ops")?;
        let mut ops = Vec::with_capacity(num_ops as usize);
        while num_ops > 0 {
            let (opcode, num_bytes) = bijoux::u64::decode(&data[offset..]).wrap_err("failed to decode opcode")?;
            num_ops -= 1;
            offset += num_bytes;
            match opcode {
                0 => {
                    let (value_len, num_bytes) = bijoux::u64::decode(&data[offset..])?;
                    offset += num_bytes;
                    let value_bytes = &data[offset..value_len as usize];
                    offset += value_len as usize;
                    let value = str::from_utf8(&value_bytes).wrap_err("failed to parse key")?.to_string();
                    ops.push(Op::Add{value}); 
                }
                1 => {
                    let (mut pred_len, num_bytes) = bijoux::u64::decode(&data[offset..]).wrap_err("failed to decode pred len")?;
                    offset += num_bytes;
                    let mut preds = Vec::with_capacity(pred_len as usize);
                    while pred_len > 0 {
                        let pred_bytes = &data[offset..offset + 32];
                        preds.push(CommitHash::try_from(pred_bytes).wrap_err("unable to parse pred")?);
                        offset += 32;
                        pred_len -= 1;
                    }
                    ops.push(Op::Remove{predecessors: preds})
                },
                other => return Err(eyre!("unknown op code {}", other))
            }
        }
        Ok(Self { ops })
    }
}

struct State {
    adds: HashMap<String, BTreeSet<CommitHash>>,
    removes: BTreeSet<CommitHash>,
}

impl State {
    fn finish(&self) -> BTreeSet<String> {
        let mut result = BTreeSet::new();
        for (key, commits) in &self.adds {
            let remaining_adds = commits.difference(&self.removes);      
            if remaining_adds.count() > 0 {
                result.insert(key.to_string());
            }
        }
        result
    }
}

fn evaluate_orset(graph: &CommitGraph<OrSetCommit>) -> BTreeSet<String> {
    let mut state = State {
        adds: HashMap::new(),
        removes: BTreeSet::new(),
    };
    graph.traverse(|meta,payload| {
        for op in &payload.ops {
            match op {
                Op::Add { value } => {
                    state.adds.entry(value.to_string()).or_default().insert(*meta.hash());
                }
                Op::Remove { predecessors } => {
                    state.removes.extend(predecessors);
                }
            }
        }
        ControlFlow::Continue(())
    });
    state.finish()
}


#[test]
fn test_basic() {
    let mut commit_graph = CommitGraph::<OrSetCommit>::new();

    let first_commit = OrSetCommit {
        ops: vec![Op::Add{value: "foo".to_string()}],
    };
    let first_commit_meta = CommitMeta::new(CommitContents {
        parents: Vec::new(),
        author: None,
        signatures: Vec::new(),
        payload: BlobHash::of_contents(&first_commit.encode()),
    });

    let second_commit = OrSetCommit {
        ops: vec![Op::Add{value: "foo".to_string()}],
    };
    let second_commit_meta = CommitMeta::new(CommitContents {
        parents: vec![*first_commit_meta.hash()],
        author: None,
        signatures: Vec::new(),
        payload: BlobHash::of_contents(&second_commit.encode()),
    });


    commit_graph.ingest_commit(first_commit_meta, first_commit);
    commit_graph.ingest_commit(second_commit_meta, second_commit);

    let result = evaluate_orset(&commit_graph);
    assert_eq!(result, BTreeSet::from_iter(["foo".to_string()]));

}
