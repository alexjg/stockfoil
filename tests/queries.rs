//! The codec itself need not be cloneable to construct, mutate, or query a graph.
use std::{collections::HashMap, ops::ControlFlow};

use ed25519_dalek::{Signer, SigningKey};
use stockfoil::*;

struct ByteCodec;

impl PayloadCodec for ByteCodec {
    type Payload = u8;
    type Error = std::io::Error;

    fn format_id(&self) -> &str {
        "test/byte/v1"
    }

    fn encode_commit(&self, payload: &u8) -> Result<Vec<u8>, Self::Error> {
        Ok(vec![*payload])
    }

    fn decode_commit(&self, bytes: &[u8], limits: &DecodeLimits) -> Result<u8, Self::Error> {
        match bytes {
            [value] if limits.max_payload_bytes >= 1 => Ok(*value),
            _ => Err(std::io::Error::other("expected one byte")),
        }
    }

    fn encode_fragment(&self, _: &FragmentShape, payloads: &[&u8]) -> Result<Vec<u8>, Self::Error> {
        Ok(payloads.iter().map(|value| **value).collect())
    }

    fn decode_fragment(
        &self,
        shape: &FragmentShape,
        bytes: &[u8],
        limits: &DecodeLimits,
    ) -> Result<Vec<u8>, Self::Error> {
        if bytes.len() != shape.members.len() || bytes.len() > limits.max_payload_bytes {
            return Err(std::io::Error::other("invalid byte batch"));
        }
        Ok(bytes.to_vec())
    }
}

#[test]
fn nonclone_codec_supports_direct_queries_materialization_and_encoded_merge() {
    let key = SigningKey::from_bytes(&[71; 32]);
    let author = Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap();
    let mut graph = CommitGraph::new(ByteCodec);
    graph.set_write_permissions(&HashMap::from([(author, WritePermission::Unbounded)]));
    let before = graph.view_point();
    let root = graph.commit(author, 1).unwrap();
    let tip = graph.commit(author, 2).unwrap();
    let after = graph.view_point();

    assert_eq!(graph.roots().copied().collect::<Vec<_>>(), [root.hash]);
    assert_eq!(graph.heads().collect::<Vec<_>>(), [tip.hash]);
    assert_eq!(
        graph.parents(&tip.hash).copied().collect::<Vec<_>>(),
        [root.hash]
    );
    assert_eq!(
        graph.children(&root.hash).copied().collect::<Vec<_>>(),
        [tip.hash]
    );
    assert!(graph.is_ancestor(&root.hash, &tip.hash));
    let diff: ViewPointDiff<'_, ByteCodec> = graph.diff(&before, &after).unwrap();
    assert_eq!(diff.visibility(&root.hash), Ok(Vis::After));
    assert_eq!(diff.changed_commits().count(), 2);

    let mut delivered = Vec::new();
    let cursor = graph
        .visit_changes(&MaterializationCursor::new(), &after, |change| {
            assert!(change.newly_delivered);
            assert_eq!(change.visibility, Vis::After);
            delivered.push((*change.meta.hash(), *change.payload));
        })
        .unwrap();
    assert_eq!(delivered, [(root.hash, 1), (tip.hash, 2)]);
    graph
        .visit_changes(&cursor, &after, |_| panic!("already delivered"))
        .unwrap();
    let mut suffix = Vec::new();
    graph.traverse_since(&[root.hash], |meta, payload| {
        suffix.push((*meta.hash(), *payload));
        ControlFlow::Continue(())
    });
    assert_eq!(suffix, [(tip.hash, 2)]);

    for draft in [&tip, &root] {
        graph
            .ingest_signature(
                author,
                draft.hash,
                Signature::from(key.sign(&draft.signing_bytes).to_bytes()),
            )
            .unwrap();
    }
    assert_eq!(graph.signed_view_point(), after);
    assert!(graph.has_commit(&root.hash));
    assert_eq!(graph.pending_items(), 0);
    assert_eq!(graph.missing_dependencies().count(), 0);
    let mut peer = CommitGraph::new(ByteCodec);
    peer.merge(&graph).unwrap();
    assert!(peer.has_commit(&root.hash));
    assert!(peer.has_commit(&tip.hash));
    let mut received = Vec::new();
    peer.traverse(|meta, payload| {
        received.push((*meta.hash(), *payload));
        ControlFlow::Continue(())
    });
    assert_eq!(received, delivered);
}
