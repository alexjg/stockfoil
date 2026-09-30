use super::*;
type E = std::convert::Infallible;
fn put(out: &mut Vec<u8>, n: usize) {
    super::put::<E>(out, n).unwrap();
}
fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
    super::blob::<E>(out, bytes).unwrap();
}
#[test]
fn local_framing_checks_wire_counts_without_decode_budgets() {
    assert_eq!(wire_u32::<E>(u32::MAX as usize).unwrap(), u32::MAX);
    if let Some(too_large) = (u32::MAX as usize).checked_add(1) {
        let mut bytes = vec![];
        assert!(matches!(
            super::put::<E>(&mut bytes, too_large),
            Err(Error::InvalidEncoding(_))
        ));
        assert!(bytes.is_empty());
    }
}
fn columns() -> [Vec<u8>; 5] {
    [
        integers([0, 0]),
        integers([1, 3]),
        booleans([false, true, false, true]),
        integers([1, 1]),
        integers([0, 0]),
    ]
}
fn envelope(n: usize, authors: usize, external: usize, columns: &[Vec<u8>; 5]) -> Vec<u8> {
    let mut out = b"STKF\x01\x01".to_vec();
    blob(&mut out, b"test");
    out.extend([0; 32]);
    put(&mut out, 0);
    put(&mut out, n);
    put(&mut out, authors);
    for i in 0..authors {
        out.extend([i as u8; 32]);
    }
    put(&mut out, external);
    for i in 0..external {
        out.extend([i as u8; 32]);
    }
    for c in columns {
        blob(&mut out, c);
    }
    put(&mut out, 0);
    blob(&mut out, &[]);
    blob(&mut out, &[]);
    blob(&mut out, &[]);
    out
}
fn parse_test(bytes: &[u8]) -> Result<Parsed<'_>, Error<E>> {
    parse(bytes, "test", &DecodeLimits::default())
}
#[test]
fn ordered_duplicate_parents_and_fixed_hexane_vectors() {
    assert_eq!(integers([0, 0]), [2, 0]);
    assert_eq!(integers([1, 3]), [0x7e, 1, 3]);
    assert_eq!(booleans([false, true, false, true]), [1, 1, 1, 1]);
    let bytes = envelope(2, 1, 1, &columns());
    let parsed = parse_test(&bytes).unwrap();
    assert_eq!(
        parsed.shape.members[0].parents,
        [ParentRef::External([0; 32].into())]
    );
    assert_eq!(
        parsed.shape.members[1].parents,
        [
            ParentRef::Member(0),
            ParentRef::External([0; 32].into()),
            ParentRef::Member(0)
        ]
    );
}
#[test]
fn malformed_hexane_columns_are_fallible_bounded_and_exact() {
    for bytes in [
        vec![1, 0],
        vec![0, 1],
        vec![0x7e, 1],
        vec![0x7e, 1, 1],
        vec![2, 1, 2, 1],
        vec![0x7f, 1, 0x7f, 2],
        vec![0xff; 20],
    ] {
        assert!(read_integers::<E>(&bytes, 2).is_err(), "{bytes:?}");
    }
    // Tiny streams must not expand huge runs or overflow Hexane's bookkeeping,
    // including alpha.5's signed-MIN negation path and wasm usize truncation.
    for count in [i64::MIN, i64::MAX, 1_000_000_000] {
        let mut bytes = Leb128::encode_signed(count).as_bytes().to_vec();
        bytes.push(0);
        assert!(read_integers::<E>(&bytes, 2).is_err());
    }
    let huge = Leb128::encode_unsigned(u64::MAX).as_bytes().to_vec();
    assert!(read_booleans::<E>(&huge, 2).is_err());
    for bytes in [vec![0], vec![1, 0], vec![0, 2, 0], vec![0x80, 0]] {
        assert!(read_booleans::<E>(&bytes, 2).is_err());
    }
    assert!(read_integers::<E>(&integers([1, 2, 3]), 2).is_err());
    assert!(read_integers::<E>(&integers([1]), 2).is_err());
}
#[test]
fn counts_dictionary_indices_distances_and_limits_are_validated_before_codec() {
    for (column, data) in [
        (0, integers([0, 1])),
        (1, integers([1, usize::MAX.min(u32::MAX as usize)])),
        (2, booleans([false, true])),
        (3, integers([0, 1])),
        (3, integers([2, 1])),
        (4, integers([0, 1])),
    ] {
        let mut c = columns();
        c[column] = data;
        assert!(
            parse_test(&envelope(2, 1, 1, &c)).is_err(),
            "column {column}"
        );
    }
    let bytes = envelope(2, 1, 1, &columns());
    for limits in [
        DecodeLimits {
            max_members: 1,
            ..DecodeLimits::default()
        },
        DecodeLimits {
            max_parents: 3,
            ..DecodeLimits::default()
        },
        DecodeLimits {
            max_item_bytes: 4,
            ..DecodeLimits::default()
        },
    ] {
        assert!(matches!(
            parse::<E>(&bytes, "test", &limits),
            Err(Error::LimitExceeded(_))
        ));
    }
    for i in 0..bytes.len() {
        assert!(parse_test(&bytes[..i]).is_err());
    }
}
#[test]
fn complete_chain_metadata_and_backward_distance_comparison() {
    let n = 10_000;
    let cols = [
        integers(std::iter::repeat_n(0, n)),
        integers([0].into_iter().chain(std::iter::repeat_n(1, n - 1))),
        booleans(std::iter::repeat_n(true, n - 1)),
        integers(std::iter::repeat_n(1, n - 1)),
        vec![],
    ];
    let bytes = envelope(n, 1, 0, &cols);
    assert!(
        bytes.len() < 115,
        "all envelope framing, hash, dictionary, empty body/signatures: {}",
        bytes.len()
    );
    assert_eq!(parse_test(&bytes).unwrap().shape.members.len(), n);
    let mut delta = hexane::leb128::DeltaEncoder::<u64>::default();
    for i in 0..n - 1 {
        delta.append(i as u64);
    }
    let absolute = delta.save();
    assert!(cols[3].len() < absolute.len());
    println!(
        "chain metadata={} backward={} absolute_delta={}",
        bytes.len(),
        cols[3].len(),
        absolute.len()
    );
    let mut delta = hexane::leb128::DeltaEncoder::<u64>::default();
    let mut backwards = Vec::new();
    for i in 1..n {
        let parent = (i - 1) / 2;
        delta.append(parent as u64);
        backwards.push(i - parent);
    }
    let backward = integers(backwards);
    let absolute = delta.save();
    println!(
        "binary tree backward={} absolute_delta={}",
        backward.len(),
        absolute.len()
    );
    assert!(backward.len() < 16000);
    assert!(absolute.len() < 11000);
    // Neither layout dominates all shapes; backwards are chosen for the chain
    // and local-neighbour workload, with no signed delta domain on receipt.
}
#[test]
fn mixed_author_branching_complete_metadata_size() {
    let n = 1000;
    let cols = [
        integers((0..n).map(|i| i % 4)),
        integers([0].into_iter().chain(std::iter::repeat_n(2, n - 1))),
        booleans(std::iter::repeat_n(true, 2 * (n - 1))),
        integers((1..n).flat_map(|i| [1, i - (i - 1) / 2])),
        vec![],
    ];
    let bytes = envelope(n, 4, 0, &cols);
    assert!(bytes.len() < 5000, "{}", bytes.len());
    let shape = parse_test(&bytes).unwrap().shape;
    assert_eq!(
        shape.members[1].parents,
        [ParentRef::Member(0), ParentRef::Member(0)]
    );
    assert_eq!(
        shape.members[999].parents,
        [ParentRef::Member(998), ParentRef::Member(499)]
    );
    println!("mixed-author branching complete metadata={}", bytes.len());
}
#[test]
fn bounded_random_column_bytes_never_panic() {
    let mut state = 0x123456789u64;
    for len in 0..32 {
        for _ in 0..1000 {
            let bytes: Vec<_> = (0..len)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state as u8
                })
                .collect();
            let _ = read_integers::<E>(&bytes, 8);
            let _ = read_booleans::<E>(&bytes, 8);
        }
    }
}
