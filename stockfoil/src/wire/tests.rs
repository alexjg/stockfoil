use super::*;
use crate::{Author, Signature};

fn format() -> FormatId {
    FormatId::new("test/v1").unwrap()
}

fn meta() -> BundleMeta {
    let a = Author::try_from(&[1u8; 32][..]).unwrap();
    let b = Author::try_from(&[2u8; 32][..]).unwrap();
    let x = CommitHash::from([0x10; 32]);
    let y = CommitHash::from([0x20; 32]);
    BundleMeta {
        id: ItemId::Fragment(FragmentRange {
            head: CommitHash::from([0; 32]),
            boundary: [x].into_iter().collect(),
        }),
        members: vec![
            MemberShape {
                author: a,
                parents: vec![ParentRef::External(x), ParentRef::External(y)],
            },
            MemberShape {
                author: b,
                parents: vec![
                    ParentRef::Member(0),
                    ParentRef::Member(0),
                    ParentRef::External(y),
                ],
            },
            MemberShape {
                author: a,
                parents: vec![],
            },
        ],
        evidence: vec![
            Evidence {
                author: b,
                member: 1,
                signature: Signature::from([7; 64]),
            },
            Evidence {
                author: a,
                member: 2,
                signature: Signature::from([8; 64]),
            },
        ],
    }
}

#[test]
fn header_round_trips() {
    let m = meta();
    let bytes = encode_header(&m, &format());
    assert_eq!(&bytes[..5], b"STKF\x01");
    assert_eq!(
        header_len(&bytes, &Limits::default()).unwrap(),
        Some(bytes.len())
    );
    assert_eq!(
        decode_header(&bytes, &format(), &Limits::default()).unwrap(),
        m
    );
}

#[test]
fn header_len_waits_for_its_prefix() {
    let bytes = encode_header(&meta(), &format());
    for n in 0..6 {
        assert_eq!(header_len(&bytes[..n], &Limits::default()).unwrap(), None);
    }
    for version in [0, 2, 255] {
        assert!(header_len(&[b'S', b'T', b'K', b'F', version], &Limits::default()).is_err());
    }
    assert!(header_len(b"XX", &Limits::default()).is_err());
    let small = Limits {
        max_header_bytes: 10,
        ..Limits::default()
    };
    assert_eq!(
        header_len(&bytes, &small).unwrap_err(),
        Error::Limit("header bytes")
    );
}

#[test]
fn format_is_checked_before_members() {
    let bytes = encode_header(&meta(), &format());
    let other = FormatId::new("test/v2").unwrap();
    assert_eq!(
        decode_header(&bytes, &other, &Limits::default()).unwrap_err(),
        Error::WrongFormat
    );
    let few = Limits {
        max_members: 1,
        ..Limits::default()
    };
    assert_eq!(
        decode_header(&bytes, &format(), &few).unwrap_err(),
        Error::Limit("members")
    );
    assert_eq!(
        decode_header(&bytes, &other, &few).unwrap_err(),
        Error::WrongFormat
    );
}

#[test]
fn every_truncation_and_extension_is_rejected() {
    let bytes = encode_header(&meta(), &format());
    for n in 0..bytes.len() {
        assert!(
            decode_header(&bytes[..n], &format(), &Limits::default()).is_err(),
            "{n}"
        );
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(decode_header(&longer, &format(), &Limits::default()).is_err());
}

#[test]
fn single_byte_corruptions_never_panic() {
    let bytes = encode_header(&meta(), &format());
    for i in 0..bytes.len() {
        for flip in [1u8, 0x80, 0xff] {
            let mut c = bytes.clone();
            c[i] ^= flip;
            if let Ok(m) = decode_header(&c, &format(), &Limits::default()) {
                // Whatever decodes must re-encode identically.
                assert_eq!(encode_header(&m, &format()), c);
            }
        }
    }
}

#[test]
fn standalone_commits_have_one_member_and_no_boundary() {
    let a = Author::try_from(&[1u8; 32][..]).unwrap();
    let m = BundleMeta {
        id: ItemId::Commit(CommitHash::from([3; 32])),
        members: vec![MemberShape {
            author: a,
            parents: vec![],
        }],
        evidence: vec![],
    };
    let bytes = encode_header(&m, &format());
    assert_eq!(
        decode_header(&bytes, &format(), &Limits::default()).unwrap(),
        m
    );
    let mut two = m.clone();
    two.members.push(two.members[0].clone());
    let bytes = encode_header(&two, &format());
    assert!(decode_header(&bytes, &format(), &Limits::default()).is_err());
}
