//! Arbitrary bytes into `wire::header_len` / `wire::decode_header` with small
//! limits; decoded headers must re-encode to an equal `BundleMeta`. Odd mode
//! bytes instead build a structurally valid `BundleMeta` and check
//! encode -> decode. The trailing bytes also go to the cursor parsers.
//!
//! Input: `mode, max_header, max_members, max_parents, max_signatures,
//! max_boundaries, bytes...`
#![no_main]
use arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use stockfoil::wire;
use stockfoil::*;
use stockfoil_fuzz::codec::format;

fn limits(sel: &[u8]) -> Limits {
    let pick = |v: u8, small: &[usize]| {
        if v >= 240 {
            65_536
        } else {
            small[v as usize % small.len()]
        }
    };
    Limits {
        max_header_bytes: pick(sel[0], &[0, 1, 8, 40, 64, 120, 200, 1000, 4096, 1 << 20]),
        max_members: pick(sel[1], &[0, 1, 2, 3, 4, 5, 8, 16]),
        max_parents: pick(sel[2], &[0, 1, 2, 3, 4, 6, 8, 16]),
        max_signatures: pick(sel[3], &[0, 1, 2, 3, 4, 8]),
        max_boundaries: pick(sel[4], &[0, 1, 2, 3, 4, 8]),
        max_canonical_bytes: u64::MAX,
        max_item_bytes: u64::MAX,
    }
}

/// Re-encoding a decoded header must decode to the same metadata.
fn round_trip(meta: &BundleMeta, fmt: &FormatId, limits: &Limits) {
    let enc = wire::encode_header(meta, fmt);
    let relaxed = Limits {
        max_header_bytes: usize::MAX,
        ..limits.clone()
    };
    assert_eq!(
        wire::header_len(&enc, &relaxed),
        Ok(Some(enc.len())),
        "header_len of encoded header"
    );
    for k in 0..enc.len() {
        match wire::header_len(&enc[..k], &relaxed) {
            Ok(None) => {}
            Ok(Some(n)) => assert_eq!(n, enc.len(), "prefix header_len disagrees"),
            Err(e) => panic!("prefix of a valid header is malformed: {e:?}"),
        }
    }
    let again = wire::decode_header(&enc, fmt, &relaxed)
        .unwrap_or_else(|e| panic!("re-encoded header does not decode: {e:?}\nmeta {meta:?}"));
    assert_eq!(&again, meta, "header round trip changed metadata");
    assert_eq!(
        wire::encode_header(&again, fmt),
        enc,
        "header encoding is not stable"
    );
}

fn check_limits(meta: &BundleMeta, limits: &Limits) {
    let n = meta.members.len();
    assert!(
        n >= 1 && n <= limits.max_members,
        "decoded member count outside limits"
    );
    let parents: usize = meta.members.iter().map(|m| m.parents.len()).sum();
    assert!(
        parents <= limits.max_parents,
        "decoded parent count outside limits"
    );
    assert!(meta.evidence.len() <= limits.max_signatures);
    if let ItemId::Fragment(r) = &meta.id {
        assert!(r.boundary.len() <= limits.max_boundaries);
    } else {
        assert_eq!(n, 1, "decoded commit with several members");
    }
    for (i, m) in meta.members.iter().enumerate() {
        for p in &m.parents {
            if let ParentRef::Member(j) = p {
                assert!(*j < i, "decoded member parent is not earlier");
            }
        }
    }
    for e in &meta.evidence {
        assert!(e.member < n, "decoded evidence target out of range");
    }
}

fn decode_mode(fmt: &FormatId, limits: &Limits, bytes: &[u8]) {
    let full = wire::header_len(bytes, limits);
    // Prefix monotonicity: a verdict on a prefix never changes with more bytes.
    for k in 0..=bytes.len().min(16) {
        match wire::header_len(&bytes[..k], limits) {
            Ok(None) => {}
            Ok(Some(n)) => assert_eq!(
                full,
                Ok(Some(n)),
                "header_len changed after a definite answer"
            ),
            Err(e) => assert_eq!(full, Err(e), "header_len error not stable"),
        }
    }
    let mut candidates = vec![bytes];
    if let Ok(Some(n)) = full
        && n < bytes.len()
    {
        candidates.push(&bytes[..n]);
    }
    for c in candidates {
        if let Ok(meta) = wire::decode_header(c, fmt, limits) {
            assert_eq!(wire::header_len(c, limits), Ok(Some(c.len())));
            check_limits(&meta, limits);
            round_trip(&meta, fmt, limits);
        }
    }
    // Durable metadata parsers share the reader. In particular, checkpoints
    // have nested collections and must reject hostile lengths before allocating.
    if let Ok(c) = Checkpoint::from_bytes(bytes) {
        assert_eq!(Checkpoint::from_bytes(&c.to_bytes()).as_ref(), Ok(&c));
    }
    if let Ok(c) = MaterializationCursor::from_bytes(bytes) {
        assert_eq!(
            MaterializationCursor::from_bytes(&c.to_bytes()).as_ref(),
            Ok(&c)
        );
    }
    if let Ok(c) = ExportCursor::from_bytes(bytes) {
        assert_eq!(ExportCursor::from_bytes(&c.to_bytes()).as_ref(), Ok(&c));
    }
}

fn meta_mode(u: &mut Unstructured) -> Result<()> {
    let authors: Vec<[u8; 32]> = (0..u.int_in_range(1..=4)?)
        .map(|_| u.arbitrary())
        .collect::<Result<_>>()?;
    let hashes: Vec<[u8; 32]> = (0..u.int_in_range(1..=4)?)
        .map(|_| u.arbitrary())
        .collect::<Result<_>>()?;
    let pick_author = |u: &mut Unstructured| -> Result<Author> {
        Ok(Author::try_from(&authors[u.choose_index(authors.len())?][..]).unwrap())
    };
    let pick_hash = |u: &mut Unstructured| -> Result<CommitHash> {
        Ok(CommitHash::from(hashes[u.choose_index(hashes.len())?]))
    };
    let fragment: bool = u.arbitrary()?;
    let n = if fragment { u.int_in_range(1..=8)? } else { 1 };
    let mut members = Vec::with_capacity(n);
    for i in 0..n {
        let mut parents = vec![];
        for _ in 0..u.int_in_range(0..=4)? {
            parents.push(if i > 0 && u.arbitrary()? {
                ParentRef::Member(u.int_in_range(0..=i - 1)?)
            } else {
                ParentRef::External(pick_hash(u)?)
            });
        }
        members.push(MemberShape {
            author: pick_author(u)?,
            parents,
        });
    }
    let mut evidence = vec![];
    for _ in 0..u.int_in_range(0..=4)? {
        evidence.push(Evidence {
            author: pick_author(u)?,
            member: u.int_in_range(0..=n - 1)?,
            signature: Signature::from(u.arbitrary::<[u8; 64]>()?),
        });
    }
    let head = pick_hash(u)?;
    let id = if fragment {
        let mut boundary = std::collections::BTreeSet::new();
        for _ in 0..u.int_in_range(0..=3)? {
            boundary.insert(pick_hash(u)?);
        }
        ItemId::Fragment(FragmentRange { head, boundary })
    } else {
        ItemId::Commit(head)
    };
    let name: String = u.arbitrary()?;
    let fmt = FormatId::new(&name).unwrap_or_else(|_| format());
    let meta = BundleMeta {
        id,
        members,
        evidence,
    };
    round_trip(&meta, &fmt, &Limits::default());
    // A different expected format is rejected.
    if fmt != format() {
        let enc = wire::encode_header(&meta, &fmt);
        assert_eq!(
            wire::decode_header(&enc, &format(), &Limits::default()),
            Err(Error::WrongFormat)
        );
    }
    Ok(())
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 6 {
        return;
    }
    let (sel, bytes) = data.split_at(6);
    if sel[0] & 1 == 1 {
        let _ = meta_mode(&mut Unstructured::new(bytes));
        return;
    }
    let fmt = if sel[0] & 2 == 0 {
        format()
    } else {
        FormatId::new("x").unwrap()
    };
    decode_mode(&fmt, &limits(&sel[1..]), bytes);
});
