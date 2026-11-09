//! Actual public cursor serialization, independent of any history validity premise.
use vstd::prelude::*;
use crate::{view::MaterializationCursor, permission_map_codec as maps,
    reader::Reader, write_permissions::WritePermissions};

verus! {

pub fn public_cursor_roundtrip(cursor: &MaterializationCursor) {
    let bytes = cursor.to_bytes();
    let decoded = MaterializationCursor::from_bytes(&bytes).unwrap();
    proof { decoded.same_codec_data(cursor); }
    assert(decoded.same(cursor));
}

pub fn equal_cursors_have_identical_bytes(a: &MaterializationCursor, b: &MaterializationCursor)
    requires a.same(b),
{
    let first = a.to_bytes();
    let second = b.to_bytes();
    proof { a.encoding_deterministic(b,first@,second@); }
    assert(first@ == second@);
}

pub fn public_decode_classifies_every_input(bytes: &[u8]) {
    let result = MaterializationCursor::from_bytes(bytes);
    assert((result is Ok) == (MaterializationCursor::decode_model(bytes@) is Some));
    if let Ok(cursor) = result {
        assert(MaterializationCursor::decode_model(bytes@) == Some(cursor.codec_data()));
    }
}

pub(crate) fn map_roundtrip(permissions: &WritePermissions, suffix: &[u8]) {
    let mut bytes = Vec::new();
    maps::encode(&mut bytes,permissions);
    let ghost encoded = bytes@;
    bytes.extend_from_slice(suffix);
    proof {
        vstd::std_specs::vec::axiom_spec_len(&bytes);
        assert(maps::encoded(permissions,encoded));
        maps::encoding_decodes(permissions,encoded,suffix@);
        assert(bytes@ == encoded+suffix@);
        assert(maps::parse(bytes@) == Some((maps::values(permissions),crate::permission_codec::permission_bytes(&permissions.fallback()),suffix@)));
    }
    let mut reader = Reader::new(&bytes);
    let decoded = maps::decode(&mut reader).unwrap();
    proof {
        assert(maps::values(&decoded) == maps::values(permissions));
        assert(crate::permission_codec::permission_bytes(&decoded.fallback()) == crate::permission_codec::permission_bytes(&permissions.fallback()));
        maps::same_values(&decoded,permissions);
    }
    assert(decoded.same(permissions));
    assert(reader.unread() == suffix@);
}

pub(crate) fn malformed_map_is_rejected(bytes: &[u8])
    requires maps::parse(bytes@) is None,
{
    let mut reader = Reader::new(bytes);
    let result = maps::decode(&mut reader);
    assert(result is Err);
}
} // verus!
