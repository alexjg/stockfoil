//! Exercise the public encoded API without duplicating Stockfoil's parser.
use super::*;
use stockfoil::ItemId;

fn corrupt(bytes: &[u8], needle: &[u8]) -> Vec<u8> {
    let offset = bytes
        .windows(needle.len())
        .position(|w| w == needle)
        .unwrap();
    let mut forged = bytes.to_vec();
    forged[offset + needle.len() - 1] ^= 1;
    forged
}

#[test]
fn check_report_surfaces_deferred_rejections_even_with_admitted_progress() {
    use stockfoil::{IngestError, IngestRejection};

    check_report(&IngestReport::default()).unwrap();
    let mut doc = MvRegisterMap::new();
    let hash = doc.set("title", "valid").unwrap();
    let mut report = IngestReport {
        admitted: vec![ItemId::Commit(hash)],
        pending: 1,
        ..IngestReport::default()
    };
    check_report(&report).unwrap();
    report.rejected.push(IngestRejection {
        item: ItemId::Commit(hash),
        error: IngestError::InvalidFragment,
    });
    let error = check_report(&report).unwrap_err().to_string();
    assert!(error.contains("queued items rejected"));
    assert!(error.contains(&format!("{:?}", report.rejected[0])));
    // Checking the outcome must not discard admitted progress or its report.
    assert_eq!(report.admitted, vec![ItemId::Commit(hash)]);
    assert_eq!(doc.get("title"), BTreeSet::from(["valid".into()]));
}

#[test]
fn local_large_assignment_exports_and_requires_raised_inbound_limits() {
    let mut source = MvRegisterMap::new();
    let value = "x".repeat(8 * 1024 * 1024 + 1);
    let hash = source.set("k", &value).unwrap();
    assert!(source.graph.has_commit(&hash));
    assert_eq!(source.get("k"), BTreeSet::from([value.clone()]));
    let (cover, _) = source
        .graph
        .export_changes(&stockfoil::ExportCursor::default())
        .unwrap();
    assert_eq!(cover.added.len(), 1);
    assert!(cover.added[0].bytes.len() > 8 * 1024 * 1024);

    // fork/merge surface inbound failures, not panics, with the core error intact.
    let error = source
        .fork()
        .err()
        .expect("default inbound budget must reject");
    assert!(
        error
            .downcast_ref::<stockfoil::MergeError<std::io::Error>>()
            .is_some()
    );
    let mut target = MvRegisterMap::new();
    target.set_write_permissions(source.view_point().write_permissions());
    target.graph.set_decode_limits(stockfoil::DecodeLimits {
        max_item_bytes: cover.added[0].bytes.len(),
        max_payload_bytes: value.len() + 6,
        ..stockfoil::DecodeLimits::default()
    });
    target.merge(&source).unwrap();
    assert_eq!(target.get("k"), source.get("k"));
}

#[test]
fn encoded_items_reject_truncation_trailing_bytes_and_forgery_atomically() {
    let mut source = MvRegisterMap::new();
    let hash = source.set("title", "hello").unwrap();
    let item = source
        .graph
        .export_changes(&stockfoil::ExportCursor::default())
        .unwrap()
        .0
        .added
        .remove(0);
    let mut target = MvRegisterMap::new();
    target.set_write_permissions(source.view_point().write_permissions());
    for len in 0..item.bytes.len() {
        assert!(target.graph.ingest(&item.bytes[..len]).is_err());
    }
    let mut trailing = item.bytes.clone();
    trailing.push(0);
    assert!(target.graph.ingest(&trailing).is_err());
    let signature = source.signing_key.sign(hash.as_bytes());
    let payload = Assignment {
        key: "title".into(),
        value: "hello".into(),
    };
    use stockfoil::{FragmentShape, MemberShape, PayloadCodec, TransportMetadata};
    let assignment = match item.metadata {
        TransportMetadata::Commit { hash: id, .. } => {
            assert_eq!(id, hash);
            RegisterCodec.encode_commit(&payload).unwrap()
        }
        TransportMetadata::Fragment { range, .. } => {
            assert_eq!(range.head, hash);
            RegisterCodec
                .encode_fragment(
                    &FragmentShape {
                        members: vec![MemberShape {
                            author: source.author(),
                            parents: vec![],
                        }],
                    },
                    &[&payload],
                )
                .unwrap()
        }
    };
    for needle in [signature.to_bytes().as_slice(), assignment.as_slice()] {
        assert!(target.graph.ingest(&corrupt(&item.bytes, needle)).is_err());
        assert!(!target.graph.has_commit(&hash));
        assert_eq!(target.graph.pending_items(), 0);
    }
    target.graph.ingest(&item.bytes).unwrap();
    target.materialize();
    assert_eq!(target.get("title"), source.get("title"));
}
