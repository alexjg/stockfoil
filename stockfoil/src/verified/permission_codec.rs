//! Production permission tags and checkpoint booleans, with exact parsing contracts.
use super::{
    api_error::Error, metadata_encoding::encode_hashes, reader::Reader,
    write_permission::WritePermission,
};
#[cfg(verus_keep_ghost)]
use super::{
    metadata_encoding::encoded_hashes,
    reader::{fits_count, prefix_count},
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

broadcast use vstd::seq::group_seq_lemmas;

pub(crate) open spec fn permission_bytes(permission: &WritePermission) -> Seq<u8> {
    match permission {
        WritePermission::Unbounded => seq![0u8],
        WritePermission::Denied => seq![1u8],
        WritePermission::Bounded(heads) => seq![2u8] + encoded_hashes(heads@),
    }
}
pub(crate) open spec fn valid_permission(bytes: Seq<u8>) -> bool {
    bytes.len() > 0 && (bytes[0] == 0 || bytes[0] == 1 || (bytes[0] == 2 && fits_count(bytes.skip(1), 32)))
}
pub(crate) open spec fn permission_length(bytes: Seq<u8>) -> int {
    if bytes.len() > 0 && bytes[0] == 2 { 9 + 32 * prefix_count(bytes.skip(1)) } else { 1 }
}
pub(crate) open spec fn parsed_permission(p: &WritePermission, bytes: Seq<u8>) -> bool {
    valid_permission(bytes) && match p {
        WritePermission::Unbounded => bytes[0] == 0,
        WritePermission::Denied => bytes[0] == 1,
        WritePermission::Bounded(heads) => bytes[0] == 2 && heads.len() == prefix_count(bytes.skip(1))
            && forall|i: int| 0 <= i < heads.len() ==> heads@[i].0@ == bytes.subrange(9+32*i,9+32*(i+1)),
    }
}

/// Successful parsing retains a canonical, exact prefix, not merely a similar rule.
pub(crate) proof fn parsed_bytes(p: &WritePermission, bytes: Seq<u8>)
    requires parsed_permission(p, bytes),
    ensures permission_bytes(p) == bytes.take(permission_length(bytes)),
        1 <= permission_bytes(p).len() <= bytes.len(),
{
    if let WritePermission::Bounded(heads) = p {
        let payload = bytes.skip(1);
        super::metadata_encoding::u64_bytes_roundtrip(payload.take(8));
        super::metadata_encoding::hash_layout(heads@);
        assert(encoded_hashes(heads@) =~= payload.take(8 + 32*heads.len())) by {
            assert forall|j: int| 0 <= j < 8 + 32*heads.len() implies
                #[trigger] encoded_hashes(heads@)[j] == payload[j] by {
                if j >= 8 {
                    let i = (j-8)/32;
                    let k = (j-8)%32;
                    assert(0 <= i < heads.len() && 0 <= k < 32 && 9+32*i+k == j+1) by(nonlinear_arith)
                        requires 8 <= j < 8+32*heads.len(), i == (j-8)/32, k == (j-8)%32;
                    assert(heads@[i].0@ == bytes.subrange(9+32*i,9+32*(i+1)));
                }
            }
        }
    }
    assert(permission_bytes(p) =~= bytes.take(permission_length(bytes)));
}

pub(crate) proof fn permission_shape(p: &WritePermission)
    ensures permission_bytes(p).len() >= 1,
        permission_bytes(p)[0] == match p { WritePermission::Unbounded => 0u8, WritePermission::Denied => 1u8, WritePermission::Bounded(_) => 2u8 },
{
    if let WritePermission::Bounded(heads) = p {
        vstd::std_specs::vec::axiom_spec_len(heads);
        super::metadata_encoding::hash_layout(heads@);
    }
}

pub(crate) proof fn permission_bytes_injective(a: &WritePermission, b: &WritePermission)
    ensures (permission_bytes(a) == permission_bytes(b)) == a.same(b),
{
    permission_shape(a);
    permission_shape(b);
    match (a,b) {
        (WritePermission::Bounded(x), WritePermission::Bounded(y)) => {
            vstd::std_specs::vec::axiom_spec_len(x);
            vstd::std_specs::vec::axiom_spec_len(y);
            super::metadata_encoding::hash_layout(x@);
            super::metadata_encoding::hash_layout(y@);
            if permission_bytes(a) == permission_bytes(b) {
                assert(permission_bytes(a).skip(1) =~= encoded_hashes(x@));
                assert(permission_bytes(b).skip(1) =~= encoded_hashes(y@));
                assert(encoded_hashes(x@) == encoded_hashes(y@));
                assert(x.len() == y.len());
                assert forall|i: int| 0 <= i < x.len() implies x@[i] == y@[i] by {
                    assert(x@[i].0@ == y@[i].0@);
                    assert(x@[i].0 =~= y@[i].0);
                }
                assert(x@ =~= y@);
            }
        },
        _ => {},
    }
}

pub(crate) proof fn permission_layout(p: &WritePermission, suffix: Seq<u8>)
    requires (permission_bytes(p)+suffix).len() <= usize::MAX,
    ensures parsed_permission(p,permission_bytes(p)+suffix),
        permission_length(permission_bytes(p)+suffix) == permission_bytes(p).len(),
{
    let bytes = permission_bytes(p)+suffix;
    permission_shape(p);
    if let WritePermission::Bounded(heads) = p {
        vstd::std_specs::vec::axiom_spec_len(heads);
        super::metadata_encoding::hash_layout(heads@);
        assert(fits_count(bytes.skip(1),32)) by(nonlinear_arith)
            requires bytes.len() == 9+32*heads.len()+suffix.len(), bytes.skip(1).len() == bytes.len()-1,
                prefix_count(bytes.skip(1)) == heads.len(), heads.len() <= usize::MAX;
        assert forall|i: int| 0 <= i < heads.len() implies heads@[i].0@ == bytes.subrange(9+32*i,9+32*(i+1)) by {
            assert(encoded_hashes(heads@).subrange(8+32*i,8+32*(i+1)) == heads@[i].0@);
        }
    }
}

pub(crate) fn encode_permission(out: &mut Vec<u8>, permission: &WritePermission)
    ensures final(out)@ == old(out)@ + permission_bytes(permission),
{
    match permission {
        WritePermission::Unbounded => out.push(0),
        WritePermission::Denied => out.push(1),
        WritePermission::Bounded(frontier) => { out.push(2); encode_hashes(out, frontier); },
    }
}

pub(crate) fn decode_permission(r: &mut Reader<'_>) -> (out: Result<WritePermission, Error>)
    ensures match out {
        Ok(permission) => parsed_permission(&permission, old(r).unread())
            && final(r).unread() == old(r).unread().skip(permission_length(old(r).unread())),
        Err(e) => e is Malformed && !valid_permission(old(r).unread()),
    },
{
    let ghost original = r.unread();
    Ok(match r.take(1)?[0] {
        0 => WritePermission::Unbounded,
        1 => WritePermission::Denied,
        2 => {
            let heads = r.hashes()?;
            proof {
                assert forall|i: int| 0 <= i < heads.len() implies heads@[i].0@ == original.subrange(9+32*i,9+32*(i+1)) by {
                    assert(heads@[i].0@ == original.skip(1).subrange(8+32*i,8+32*(i+1)));
                }
            }
            WritePermission::Bounded(heads)
        },
        _ => return Err(Error::Malformed("view permission")),
    })
}

pub(crate) fn read_bool(r: &mut Reader<'_>) -> (out: Result<bool, Error>)
    ensures final(r).unread() == if old(r).unread().len() == 0 { old(r).unread() } else { old(r).unread().skip(1) },
        match out {
            Ok(value) => old(r).unread().len() > 0 && old(r).unread()[0] == (if value { 1u8 } else { 0u8 }),
            Err(e) => e is Malformed && (old(r).unread().len() == 0 || old(r).unread()[0] > 1),
        },
{
    match r.take(1)?[0] {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::Malformed("checkpoint boolean")),
    }
}
} // verus!
