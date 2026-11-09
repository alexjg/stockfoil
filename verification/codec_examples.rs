//! Consumers of the actual metadata codecs: arbitrary values, suffixes and malformed input.
use vstd::prelude::*;
use crate::{commit_hash::CommitHash, reader::{Reader, prefix_count, fits_count},
    metadata_encoding::{encode_u64, encode_hashes, hash_layout, u64_roundtrip},
    permission_codec::{encode_permission, decode_permission, read_bool},
    write_permission::WritePermission};

verus! {

pub(crate) fn integer_roundtrip(n: u64, suffix: &[u8]) {
    let mut bytes = Vec::new();
    encode_u64(&mut bytes, n);
    bytes.extend_from_slice(suffix);
    proof { u64_roundtrip(n); }
    let mut reader = Reader::new(&bytes);
    let decoded = reader.u64_le().unwrap();
    assert(decoded == n);
    assert(reader.unread() == suffix@);
}

pub(crate) fn hashes_roundtrip(hashes: &[CommitHash], suffix: &[u8]) {
    let mut bytes = Vec::new();
    encode_hashes(&mut bytes, hashes);
    bytes.extend_from_slice(suffix);
    proof {
        hash_layout(hashes@);
        assert(fits_count(bytes@, 32)) by(nonlinear_arith)
            requires bytes@.len() == 8 + 32*hashes.len() + suffix.len(),
                prefix_count(bytes@) == hashes.len(), hashes.len() <= usize::MAX;
    }
    let mut reader = Reader::new(&bytes);
    let decoded = reader.hashes().unwrap();
    assert forall|i: int| 0 <= i < hashes.len() implies decoded@[i] == hashes@[i] by {
        assert(decoded@[i].0 =~= hashes@[i].0);
    }
    assert(decoded@ =~= hashes@);
    assert(reader.unread() == suffix@);
}

pub(crate) fn permission_roundtrip(permission: &WritePermission, suffix: &[u8]) {
    let mut bytes = Vec::new();
    encode_permission(&mut bytes, permission);
    bytes.extend_from_slice(suffix);
    proof {
        if let WritePermission::Bounded(heads) = permission {
            vstd::std_specs::vec::axiom_spec_len(heads);
            hash_layout(heads@);
            assert(fits_count(bytes@.skip(1), 32)) by(nonlinear_arith)
                requires bytes@.len() == 9 + 32*heads.len() + suffix.len(),
                    bytes@.skip(1).len() == bytes@.len()-1,
                    prefix_count(bytes@.skip(1)) == heads.len(), heads.len() <= usize::MAX;
        }
    }
    let mut reader = Reader::new(&bytes);
    let decoded = decode_permission(&mut reader).unwrap();
    proof {
        if let WritePermission::Bounded(heads) = permission {
            let decoded_heads = decoded->Bounded_0;
            assert forall|i: int| 0 <= i < heads.len() implies decoded_heads@[i] == heads@[i] by {
                assert(decoded_heads@[i].0 =~= heads@[i].0);
            }
            assert(decoded_heads@ =~= heads@);
        }
    }
    assert(decoded.same(permission));
    assert(reader.unread() == suffix@);
}

pub(crate) fn short_reads_leave_input(bytes: &[u8], n: usize)
    requires n > bytes.len(),
{
    let mut reader = Reader::new(bytes);
    let result = reader.take(n);
    assert(result is Err);
    assert(reader.unread() == bytes@);
}

pub(crate) fn collection_bound_is_exact(bytes: &[u8], width: usize)
    requires width > 0,
{
    let mut reader = Reader::new(bytes);
    let result = reader.count(width);
    assert((result is Ok) == fits_count(bytes@, width as int));
    if let Ok(n) = result {
        assert(n <= reader.unread().len() / (width as nat));
    }
}

pub(crate) fn invalid_tags_and_trailing_data(tag: u8, suffix: &[u8])
    requires tag > 2,
{
    let mut bytes = vec![tag];
    bytes.extend_from_slice(suffix);
    let mut reader = Reader::new(&bytes);
    let result = decode_permission(&mut reader);
    assert(result is Err);
    let mut reader = Reader::new(&bytes);
    let result = read_bool(&mut reader);
    assert(result is Err);
    assert(reader.unread() == suffix@);
    let reader = Reader::new(&bytes);
    let result = reader.finish();
    assert(result is Err);
}

pub(crate) fn truncated_hash_list_is_rejected(bytes: &[u8])
    requires !fits_count(bytes@, 32),
{
    let mut reader = Reader::new(bytes);
    let result = reader.hashes();
    assert(result is Err);
}
} // verus!
