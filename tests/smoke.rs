use std::{collections::{BTreeSet}};

use stockfoil::{BlobHash, CommitContents, CommitGraph, CommitMeta};

mod orset;


#[test]
fn test_basic() {
    let mut commit_graph = CommitGraph::<orset::OrSetCommit>::new();

    let first_commit = orset::OrSetCommit {
        ops: vec![orset::Op::Add{value: "foo".to_string()}],
    };
    let first_commit_meta = CommitMeta::new(CommitContents {
        parents: Vec::new(),
        author: None,
        signatures: Vec::new(),
        payload: BlobHash::of_contents(&first_commit.encode()),
    });

    let second_commit = orset::OrSetCommit {
        ops: vec![orset::Op::Add{value: "foo".to_string()}],
    };
    let second_commit_meta = CommitMeta::new(CommitContents {
        parents: vec![*first_commit_meta.hash()],
        author: None,
        signatures: Vec::new(),
        payload: BlobHash::of_contents(&second_commit.encode()),
    });


    commit_graph.ingest_commit(first_commit_meta, first_commit);
    commit_graph.ingest_commit(second_commit_meta, second_commit);

    let result = orset::evaluate_orset(&commit_graph);
    assert_eq!(result, BTreeSet::from_iter(["foo".to_string()]));

}
