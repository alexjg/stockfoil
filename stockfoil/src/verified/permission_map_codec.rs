//! Canonical permission-map encoding and exact structural decoding.
use super::{
    api_error::Error,
    author::Author,
    metadata_encoding::encode_u64,
    permission_codec::{decode_permission, encode_permission},
    reader::Reader,
    write_permissions::WritePermissions,
};
#[cfg(verus_keep_ghost)]
use super::{
    commit_hash::CommitHash,
    permission_codec::{permission_bytes, permission_length, valid_permission},
    reader::{fits_count, prefix_count},
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

broadcast use {vstd::seq::group_seq_lemmas, vstd::seq_lib::group_seq_properties, vstd::seq_lib::group_seq_lib_default};

pub(crate) open spec fn values(p: &WritePermissions) -> Map<Author, Seq<u8>> {
    Map::new(p.authors().dom(), |a: Author| permission_bytes(&p.authors()[a]))
}
pub(crate) open spec fn keys_match(keys: Seq<CommitHash>, p: &WritePermissions) -> bool {
    keys.no_duplicates() && super::stdlib::hashes_sorted(keys)
        && forall|h: CommitHash| keys.contains(h) == p.authors().contains_key(Author(h.0))
}
pub(crate) open spec fn entries_bytes(keys: Seq<CommitHash>, map: Map<Author, Seq<u8>>) -> Seq<u8>
    decreases keys.len(),
{
    if keys.len() == 0 { Seq::empty() }
    else { keys[0].0@ + map[Author(keys[0].0)] + entries_bytes(keys.skip(1), map) }
}
pub(crate) open spec fn encoded(p: &WritePermissions, bytes: Seq<u8>) -> bool {
    exists|keys: Seq<CommitHash>| keys_match(keys,p) && keys.len() <= usize::MAX
        && bytes == vstd::bytes::spec_u64_to_le_bytes(keys.len() as u64)
            + entries_bytes(keys,values(p)) + permission_bytes(&p.fallback())
}

pub(crate) open spec fn author_at(bytes: Seq<u8>) -> Author {
    choose|a: Author| a.0@ == bytes.take(32)
}
pub(crate) proof fn author_at_unique(a: Author, bytes: Seq<u8>)
    requires bytes.len() >= 32, a.0@ == bytes.take(32),
    ensures author_at(bytes) == a,
{
    assert(author_at(bytes).0@ == a.0@);
    assert(author_at(bytes).0 =~= a.0);
}

pub(crate) open spec fn parse_entries(bytes: Seq<u8>, count: nat, accumulated: Map<Author, Seq<u8>>)
    -> Option<(Map<Author, Seq<u8>>, Seq<u8>)>
    decreases count,
{
    if count == 0 { Some((accumulated, bytes)) }
    else if bytes.len() < 32 || !valid_permission(bytes.skip(32)) { None }
    else {
        let author = author_at(bytes);
        let tail = bytes.skip(32);
        let size = permission_length(tail);
        if accumulated.contains_key(author) { None }
        else { parse_entries(tail.skip(size), (count-1) as nat, accumulated.insert(author, tail.take(size))) }
    }
}
pub(crate) open spec fn parse(bytes: Seq<u8>) -> Option<(Map<Author, Seq<u8>>, Seq<u8>, Seq<u8>)> {
    if !fits_count(bytes,33) { None }
    else {
        match parse_entries(bytes.skip(8), prefix_count(bytes) as nat, Map::empty()) {
            None => None,
            Some((map, tail)) => if valid_permission(tail) {
                Some((map, tail.take(permission_length(tail)), tail.skip(permission_length(tail))))
            } else { None },
        }
    }
}

pub(crate) proof fn same_values(p: &WritePermissions, q: &WritePermissions)
    ensures (values(p) == values(q) && permission_bytes(&p.fallback()) == permission_bytes(&q.fallback())) == p.same(q),
{
    assert(values(p).dom() == p.authors().dom());
    assert(values(q).dom() == q.authors().dom());
    super::permission_codec::permission_bytes_injective(&p.fallback(),&q.fallback());
    assert forall|a: Author| p.authors().contains_key(a) && q.authors().contains_key(a) implies
        (#[trigger] values(p)[a] == values(q)[a]) == p.authors()[a].same(&q.authors()[a]) by {
        super::permission_codec::permission_bytes_injective(&p.authors()[a],&q.authors()[a]);
    }
    if p.same(q) { assert(values(p) =~= values(q)); }
    if values(p) == values(q) {
        assert forall|a: Author| p.authors().contains_key(a) implies p.authors()[a].same(&q.authors()[a]) by {
            super::permission_codec::permission_bytes_injective(&p.authors()[a],&q.authors()[a]);
            assert(values(p)[a] == values(q)[a]);
        }
    }
}

pub(crate) proof fn entries_size(keys: Seq<CommitHash>, p: &WritePermissions)
    requires forall|h: CommitHash| keys.contains(h) ==> p.authors().contains_key(Author(h.0)),
    ensures entries_bytes(keys,values(p)).len() >= 33*keys.len(),
    decreases keys.len(),
{
    if keys.len() > 0 {
        assert(keys.contains(keys[0]));
        super::permission_codec::permission_shape(&p.authors()[Author(keys[0].0)]);
        assert forall|h: CommitHash| keys.skip(1).contains(h) implies p.authors().contains_key(Author(h.0)) by {
            assert(keys.contains(h));
        }
        entries_size(keys.skip(1),p);
    }
}

pub(crate) open spec fn remaining(keys: Seq<CommitHash>, p: &WritePermissions, acc: Map<Author,Seq<u8>>) -> bool {
    &&& keys.no_duplicates()
    &&& acc.submap_of(values(p))
    &&& forall|h: CommitHash| keys.contains(h) ==> p.authors().contains_key(Author(h.0)) && !acc.contains_key(Author(h.0))
    &&& forall|a: Author| p.authors().contains_key(a) == (acc.contains_key(a) || keys.contains(CommitHash(a.0)))
}

pub(crate) proof fn entries_roundtrip(keys: Seq<CommitHash>, p: &WritePermissions,
    acc: Map<Author,Seq<u8>>, suffix: Seq<u8>)
    requires remaining(keys,p,acc), (entries_bytes(keys,values(p))+suffix).len() <= usize::MAX,
    ensures parse_entries(entries_bytes(keys,values(p))+suffix,keys.len(),acc) == Some((values(p),suffix)),
    decreases keys.len(),
{
    if keys.len() == 0 { assert(acc =~= values(p)); }
    else {
        assert(keys.contains(keys[0]));
        let author = Author(keys[0].0);
        let tail = entries_bytes(keys.skip(1),values(p))+suffix;
        let bytes = entries_bytes(keys,values(p))+suffix;
        assert(bytes.take(32) =~= author.0@);
        author_at_unique(author,bytes);
        super::permission_codec::permission_layout(&p.authors()[author],tail);
        let next = acc.insert(author,values(p)[author]);
        assert forall|h: CommitHash| keys.skip(1).contains(h) implies p.authors().contains_key(Author(h.0)) && !next.contains_key(Author(h.0)) by {
            assert(keys.contains(h));
            assert(h != keys[0]);
        }
        assert forall|a: Author| p.authors().contains_key(a) == (next.contains_key(a) || keys.skip(1).contains(CommitHash(a.0))) by {
            if keys.contains(CommitHash(a.0)) && a != author {
                let j = choose|j: int| 0 <= j < keys.len() && keys[j] == CommitHash(a.0);
                assert(j > 0);
                assert(keys.skip(1)[j-1] == CommitHash(a.0));
            }
        }
        entries_roundtrip(keys.skip(1),p,next,suffix);
        assert(bytes.skip(32) == permission_bytes(&p.authors()[author])+tail);
        assert(bytes.skip(32).take(permission_length(bytes.skip(32))) =~= values(p)[author]);
        assert(bytes.skip(32).skip(permission_length(bytes.skip(32))) =~= tail);
        reveal(parse_entries);
    }
}

pub(crate) proof fn encoding_decodes(p: &WritePermissions, bytes: Seq<u8>, suffix: Seq<u8>)
    requires encoded(p,bytes), (bytes+suffix).len() <= usize::MAX,
    ensures parse(bytes+suffix) == Some((values(p),permission_bytes(&p.fallback()),suffix)),
{
    let keys = choose|keys: Seq<CommitHash>| keys_match(keys,p) && keys.len() <= usize::MAX
        && bytes == vstd::bytes::spec_u64_to_le_bytes(keys.len() as u64)
            + entries_bytes(keys,values(p)) + permission_bytes(&p.fallback());
    entries_size(keys,p);
    super::metadata_encoding::u64_roundtrip(keys.len() as u64);
    let input = bytes+suffix;
    assert(fits_count(input,33)) by(nonlinear_arith)
        requires prefix_count(input) == keys.len(), keys.len() <= usize::MAX,
            input.len() >= 8+33*keys.len();
    assert(remaining(keys,p,Map::empty()));
    entries_roundtrip(keys,p,Map::empty(),permission_bytes(&p.fallback())+suffix);
    super::permission_codec::permission_layout(&p.fallback(),suffix);
    assert(input.skip(8) =~= entries_bytes(keys,values(p))+permission_bytes(&p.fallback())+suffix);
    assert((permission_bytes(&p.fallback())+suffix).take(permission_length(permission_bytes(&p.fallback())+suffix)) =~= permission_bytes(&p.fallback()));
    assert((permission_bytes(&p.fallback())+suffix).skip(permission_length(permission_bytes(&p.fallback())+suffix)) =~= suffix);
    assert(input.skip(8) =~= entries_bytes(keys,values(p))+(permission_bytes(&p.fallback())+suffix));
    assert(prefix_count(input) == keys.len());
    assert(parse_entries(input.skip(8),prefix_count(input) as nat,Map::empty()) == Some((values(p),permission_bytes(&p.fallback())+suffix)));
    assert(valid_permission(permission_bytes(&p.fallback())+suffix));
    reveal(parse);
}

/// A sorted list of unique byte keys is fixed by its set of authors.
pub(crate) proof fn keys_unique(a: Seq<CommitHash>, b: Seq<CommitHash>)
    requires a.no_duplicates(), b.no_duplicates(), super::stdlib::hashes_sorted(a), super::stdlib::hashes_sorted(b),
        a.to_set() == b.to_set(),
    ensures a == b,
    decreases a.len(),
{
    a.to_set_ensures(); b.to_set_ensures();
    if a.len() == 0 || b.len() == 0 {
        if a.len() > 0 { assert(b.to_set().contains(a[0])); }
        if b.len() > 0 { assert(a.to_set().contains(b[0])); }
        assert(a =~= b);
    }
    else {
        assert(a.to_set().contains(b[0]) && b.to_set().contains(a[0]));
        assert(a.contains(b[0]) && b.contains(a[0]));
        if a[0] != b[0] {
            let i = choose|i: int| 0 <= i < a.len() && a[i] == b[0];
            let j = choose|j: int| 0 <= j < b.len() && b[j] == a[0];
            assert(i > 0 && j > 0);
            assert(super::stdlib::hash_less(a[0],b[0]) && super::stdlib::hash_less(b[0],a[0]));
            super::canonical_order::hash_asymmetric(a[0],b[0]);
        }
        assert(a.skip(1).to_set() =~= a.to_set().remove(a[0]));
        assert(b.skip(1).to_set() =~= b.to_set().remove(b[0]));
        keys_unique(a.skip(1),b.skip(1));
        assert(a =~= b);
    }
}

pub(crate) proof fn encoding_unique(p: &WritePermissions, q: &WritePermissions, a: Seq<u8>, b: Seq<u8>)
    requires p.same(q), encoded(p,a), encoded(q,b),
    ensures a == b,
{
    let ka = choose|keys: Seq<CommitHash>| keys_match(keys,p) && keys.len() <= usize::MAX
        && a == vstd::bytes::spec_u64_to_le_bytes(keys.len() as u64)+entries_bytes(keys,values(p))+permission_bytes(&p.fallback());
    let kb = choose|keys: Seq<CommitHash>| keys_match(keys,q) && keys.len() <= usize::MAX
        && b == vstd::bytes::spec_u64_to_le_bytes(keys.len() as u64)+entries_bytes(keys,values(q))+permission_bytes(&q.fallback());
    assert(ka.to_set() =~= kb.to_set());
    keys_unique(ka,kb);
    same_values(p,q);
}

pub(crate) fn encode(out: &mut Vec<u8>, permissions: &WritePermissions)
    ensures exists|bytes: Seq<u8>| encoded(permissions,bytes) && final(out)@ == old(out)@ + bytes,
{
    let keys = permissions.ordered_keys();
    let ghost prefix = out@;
    let ghost all = entries_bytes(keys@,values(permissions));
    encode_u64(out, keys.len() as u64);
    let ghost header = out@;
    assert(keys@.skip(0) =~= keys@);
    let mut i = 0usize;
    while i < keys.len()
        invariant i <= keys.len(), keys_match(keys@,permissions),
            header == prefix + vstd::bytes::spec_u64_to_le_bytes(keys.len() as u64),
            out@ + entries_bytes(keys@.skip(i as int),values(permissions)) == header + all,
        decreases keys.len()-i,
    {
        let hash = &keys[i];
        let author = Author(hash.0);
        assert(keys@.contains(*hash));
        let permission = permissions.listed(&author).unwrap();
        proof {
            assert(keys@.contains(*hash));
            assert(keys@.skip(i as int).skip(1) == keys@.skip(i+1));
            reveal(entries_bytes);
        }
        out.extend_from_slice(&author.0);
        encode_permission(out, permission);
        i += 1;
    }
    proof { reveal(entries_bytes); }
    encode_permission(out, permissions.unlisted());
    proof {
        let bytes = vstd::bytes::spec_u64_to_le_bytes(keys.len() as u64) + all + permission_bytes(&permissions.fallback());
        assert(encoded(permissions,bytes));
        assert(out@ == prefix + bytes);
    }
}

pub(crate) fn decode(r: &mut Reader<'_>) -> (out: Result<WritePermissions, Error>)
    ensures match out {
        Ok(p) => parse(old(r).unread()) == Some((values(&p),permission_bytes(&p.fallback()),final(r).unread())),
        Err(e) => e is Malformed && parse(old(r).unread()) is None,
    },
{
    let ghost original = r.unread();
    let count = r.count(33)?;
    let mut permissions = WritePermissions::new();
    assert(values(&permissions) =~= Map::<Author,Seq<u8>>::empty());
    let mut i = 0usize;
    while i < count
        invariant original == old(r).unread(), i <= count, fits_count(original,33), count == prefix_count(original),
            parse_entries(original.skip(8),count as nat,Map::empty()) == parse_entries(r.unread(),(count-i) as nat,values(&permissions)),
        decreases count-i,
    {
        let ghost before = r.unread();
        let ghost prior = values(&permissions);
        proof { reveal_with_fuel(parse_entries,2); }
        let author_bytes = match r.array::<32>() {
            Ok(bytes) => bytes,
            Err(e) => { assert(parse_entries(before,(count-i) as nat,prior) is None); assert(parse(original) is None); assert(e is Malformed); return Err(e); },
        };
        let author = Author(author_bytes);
        proof { author_at_unique(author,before); }
        let permission = match decode_permission(r) {
            Ok(p) => p,
            Err(e) => { assert(parse_entries(before,(count-i) as nat,prior) is None); assert(parse(original) is None); assert(e is Malformed); return Err(e); },
        };
        proof { super::permission_codec::parsed_bytes(&permission,before.skip(32)); }
        if permissions.insert(author,permission).is_some() {
            assert(prior.contains_key(author));
            assert(parse_entries(before,(count-i) as nat,prior) is None);
            return Err(Error::Malformed("duplicate view author"));
        }
        proof {
            assert(values(&permissions) =~= prior.insert(author,permission_bytes(&permission)));
            reveal(parse_entries);
        }
        i += 1;
    }
    proof { reveal(parse_entries); }
    let ghost tail = r.unread();
    let fallback = decode_permission(r)?;
    proof { super::permission_codec::parsed_bytes(&fallback,tail); }
    permissions.set_unlisted(fallback);
    Ok(permissions)
}
} // verus!
