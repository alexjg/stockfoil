//! Exact checkpoint rejection-set codec; preserves cascade flags and sorted keys.
use super::{
    api_error::Error, commit_hash::CommitHash, metadata_encoding::encode_u64,
    permission_codec::read_bool, reader::Reader,
};
use std::collections::BTreeMap;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::{
    prelude::*,
    std_specs::{
        cmp::{OrdSpec, PartialEqSpec, PartialOrdSpec},
        iter::IteratorSpec,
    },
};

verus! {
broadcast use {vstd::seq::group_seq_lemmas,vstd::seq_lib::group_seq_properties,vstd::seq_lib::group_seq_lib_default};

pub(crate) proof fn order_laws()
    ensures vstd::laws_cmp::obeys_cmp::<CommitHash>(),
        forall|h:CommitHash| #[trigger] h.cmp_spec(&h) is Equal,
        forall|a:CommitHash,b:CommitHash| #[trigger] a.cmp_spec(&b) is Less ==> !(b.cmp_spec(&a) is Less),
{
    super::stdlib::commit_order_model();
    reveal(vstd::laws_cmp::obeys_cmp_partial_ord);
    reveal(vstd::laws_cmp::obeys_cmp_ord);
    reveal(vstd::laws_cmp::obeys_partial_cmp_spec_properties);
    reveal(vstd::laws_eq::obeys_eq_spec_properties);
}

pub(crate) open spec fn sorted(entries: Seq<(CommitHash,bool)>) -> bool {
    forall|i:int,j:int| 0 <= i < j < entries.len() ==> entries[i].0.cmp_spec(&entries[j].0) is Less
}
pub(crate) open spec fn entries_bytes(entries: Seq<(CommitHash,bool)>) -> Seq<u8>
    decreases entries.len(),
{
    if entries.len() == 0 { Seq::empty() }
    else { entries[0].0.0@ + seq![if entries[0].1 { 1u8 } else { 0u8 }] + entries_bytes(entries.skip(1)) }
}
pub(crate) open spec fn represents(entries: Seq<(CommitHash,bool)>, map: Map<CommitHash,bool>) -> bool {
    entries.to_set() == map.kv_pairs() && entries.map_values(|e:(CommitHash,bool)| e.0).no_duplicates()
}
pub(crate) open spec fn encoded(map: Map<CommitHash,bool>, bytes: Seq<u8>) -> bool {
    exists|entries:Seq<(CommitHash,bool)>| represents(entries,map) && sorted(entries) && entries.len() <= usize::MAX
        && bytes == vstd::bytes::spec_u64_to_le_bytes(entries.len() as u64)+entries_bytes(entries)
}
pub(crate) open spec fn hash_at(bytes:Seq<u8>) -> CommitHash {
    CommitHash(super::permission_map_codec::author_at(bytes).0)
}
pub(crate) proof fn hash_at_unique(hash:CommitHash,bytes:Seq<u8>)
    requires bytes.len() >= 32, hash.0@ == bytes.take(32),
    ensures hash_at(bytes) == hash,
{ super::permission_map_codec::author_at_unique(super::author::Author(hash.0),bytes); }

pub(crate) open spec fn parse_entries(bytes:Seq<u8>,n:nat,acc:Map<CommitHash,bool>) -> Option<(Map<CommitHash,bool>,Seq<u8>)>
    decreases n,
{
    if n == 0 { Some((acc,bytes)) }
    else if bytes.len() < 33 || bytes[32] > 1 || acc.contains_key(hash_at(bytes)) { None }
    else { parse_entries(bytes.skip(33),(n-1) as nat,acc.insert(hash_at(bytes),bytes[32] == 1)) }
}
#[verifier::opaque]
pub(crate) open spec fn parse(bytes:Seq<u8>) -> Option<(Map<CommitHash,bool>,Seq<u8>)> {
    if !super::reader::fits_count(bytes,33) { None }
    else { parse_entries(bytes.skip(8),super::reader::prefix_count(bytes) as nat,Map::empty()) }
}

pub(crate) proof fn ordered_unique(a:Seq<(CommitHash,bool)>,b:Seq<(CommitHash,bool)>)
    requires sorted(a),sorted(b), a.to_set() == b.to_set(),
        a.map_values(|e:(CommitHash,bool)| e.0).no_duplicates(), b.map_values(|e:(CommitHash,bool)| e.0).no_duplicates(),
    ensures a == b,
    decreases a.len(),
{
    order_laws(); a.to_set_ensures(); b.to_set_ensures();
    if a.len() == 0 || b.len() == 0 {
        if a.len() > 0 { assert(b.to_set().contains(a[0])); }
        if b.len() > 0 { assert(a.to_set().contains(b[0])); }
        assert(a =~= b);
    } else {
        assert(a.to_set().contains(b[0]) && b.to_set().contains(a[0]));
        if a[0] != b[0] {
            let i = choose|i:int| 0 <= i < a.len() && a[i] == b[0];
            let j = choose|j:int| 0 <= j < b.len() && b[j] == a[0];
            assert(i > 0 && j > 0);
            assert(a[0].0.cmp_spec(&b[0].0) is Less && b[0].0.cmp_spec(&a[0].0) is Less);
        }
        assert(a.skip(1).to_set() =~= a.to_set().remove(a[0]));
        assert(b.skip(1).to_set() =~= b.to_set().remove(b[0]));
        assert(a.skip(1).map_values(|e:(CommitHash,bool)| e.0) =~= a.map_values(|e:(CommitHash,bool)| e.0).skip(1));
        assert(b.skip(1).map_values(|e:(CommitHash,bool)| e.0) =~= b.map_values(|e:(CommitHash,bool)| e.0).skip(1));
        ordered_unique(a.skip(1),b.skip(1));
        assert(a =~= b);
    }
}
pub(crate) proof fn encoding_unique(map:Map<CommitHash,bool>,a:Seq<u8>,b:Seq<u8>)
    requires encoded(map,a),encoded(map,b),
    ensures a == b,
{
    let x = choose|entries:Seq<(CommitHash,bool)>| represents(entries,map) && sorted(entries) && entries.len() <= usize::MAX
        && a == vstd::bytes::spec_u64_to_le_bytes(entries.len() as u64)+entries_bytes(entries);
    let y = choose|entries:Seq<(CommitHash,bool)>| represents(entries,map) && sorted(entries) && entries.len() <= usize::MAX
        && b == vstd::bytes::spec_u64_to_le_bytes(entries.len() as u64)+entries_bytes(entries);
    ordered_unique(x,y);
}

pub(crate) fn encode(out:&mut Vec<u8>,map:&BTreeMap<CommitHash,bool>)
    ensures exists|bytes:Seq<u8>| encoded(map@,bytes) && final(out)@ == old(out)@+bytes,
{
    proof { order_laws(); }
    let iterator = map.iter();
    let ghost entries = iterator.remaining().unref();
    proof {
        let keys = iterator.remaining().map_values(|kv:(&CommitHash,&bool)| *kv.0);
        vstd::std_specs::btree::axiom_increasing_seq_meaning(keys);
        assert(entries.map_values(|e:(CommitHash,bool)| e.0) =~= keys);
        assert forall|i:int,j:int| 0 <= i < j < entries.len() implies entries[i].0.cmp_spec(&entries[j].0) is Less by {
            assert(keys[i].cmp_spec(&keys[j]) is Less);
        }
        assert(sorted(entries));
        assert(entries.map_values(|e:(CommitHash,bool)| e.0) =~= keys);
        assert(keys.no_duplicates()) by {
            assert forall|i:int,j:int| 0 <= i < keys.len() && 0 <= j < keys.len() && i != j implies keys[i] != keys[j] by {
                if i < j { assert(keys[i].cmp_spec(&keys[j]) is Less); }
                else { assert(keys[j].cmp_spec(&keys[i]) is Less); }
            }
        }
        assert(represents(entries,map@));
    }
    let ghost prefix = out@;
    encode_u64(out,map.len() as u64);
    let ghost header = out@;
    for (hash,cascade) in iter: iterator
        invariant iter.seq().unref() == entries, entries.len() == map@.len(), entries.len() <= usize::MAX,
            represents(entries,map@), sorted(entries),
            header == prefix+vstd::bytes::spec_u64_to_le_bytes(entries.len() as u64),
            out@+entries_bytes(entries.skip(iter.index())) == header+entries_bytes(entries),
    {
        proof {
            assert(entries[iter.index()] == (*hash,*cascade));
            assert(entries.skip(iter.index()).skip(1) == entries.skip(iter.index()+1));
            reveal(entries_bytes);
        }
        out.extend_from_slice(&hash.0);
        out.push(if *cascade { 1 } else { 0 });
    }
    proof {
        let bytes = vstd::bytes::spec_u64_to_le_bytes(entries.len() as u64)+entries_bytes(entries);
        assert(encoded(map@,bytes));
        assert(out@ == prefix+bytes);
    }
}

pub(crate) fn decode(r:&mut Reader<'_>) -> (out:Result<BTreeMap<CommitHash,bool>,Error>)
    ensures match out {
        Ok(map) => parse(old(r).unread()) == Some((map@,final(r).unread())),
        Err(e) => e is Malformed && parse(old(r).unread()) is None,
    },
{
    proof { super::stdlib::commit_order_model(); reveal(parse); }
    let ghost original = r.unread();
    let count = r.count(33)?;
    let mut rejected:BTreeMap<CommitHash,bool> = BTreeMap::new();
    let mut i = 0usize;
    while i < count
        invariant original == old(r).unread(), i <= count, vstd::laws_cmp::obeys_cmp::<CommitHash>(),
            super::reader::fits_count(original,33), count == super::reader::prefix_count(original),
            parse_entries(original.skip(8),count as nat,Map::empty()) == parse_entries(r.unread(),(count-i) as nat,rejected@),
        decreases count-i,
    {
        let ghost before = r.unread();
        let ghost acc = rejected@;
        proof { reveal(parse); reveal_with_fuel(parse_entries,2); }
        let hash = match r.hash() {
            Ok(h) => h, Err(e) => { assert(parse_entries(before,(count-i) as nat,acc) is None); assert(parse(original) is None); return Err(e); },
        };
        proof { hash_at_unique(hash,before); }
        let cascade = match read_bool(r) {
            Ok(b) => b, Err(e) => { assert(parse_entries(before,(count-i) as nat,acc) is None); assert(parse(original) is None); return Err(e); },
        };
        if rejected.insert(hash,cascade).is_some() {
            return Err(Error::Malformed("duplicate rejected commit"));
        }
        assert(r.unread() =~= before.skip(33));
        i += 1;
    }
    proof { reveal(parse_entries); }
    Ok(rejected)
}

pub(crate) proof fn entries_size(entries:Seq<(CommitHash,bool)>)
    ensures entries_bytes(entries).len() == 33*entries.len(),
    decreases entries.len(),
{
    if entries.len() > 0 { entries_size(entries.skip(1)); }
}
pub(crate) open spec fn remaining(entries:Seq<(CommitHash,bool)>,target:Map<CommitHash,bool>,acc:Map<CommitHash,bool>) -> bool {
    &&& entries.map_values(|e:(CommitHash,bool)| e.0).no_duplicates()
    &&& acc.submap_of(target)
    &&& forall|i:int| 0 <= i < entries.len() ==> target.contains_key(entries[i].0) && target[entries[i].0] == entries[i].1 && !acc.contains_key(entries[i].0)
    &&& forall|k:CommitHash| target.contains_key(k) == (acc.contains_key(k) || entries.map_values(|e:(CommitHash,bool)| e.0).contains(k))
}
pub(crate) proof fn entries_roundtrip(entries:Seq<(CommitHash,bool)>,target:Map<CommitHash,bool>,acc:Map<CommitHash,bool>,suffix:Seq<u8>)
    requires remaining(entries,target,acc),
    ensures parse_entries(entries_bytes(entries)+suffix,entries.len(),acc) == Some((target,suffix)),
    decreases entries.len(),
{
    if entries.len() == 0 { assert(acc =~= target); }
    else {
        let tail = entries.skip(1);
        let next = acc.insert(entries[0].0,entries[0].1);
        let bytes = entries_bytes(entries)+suffix;
        assert(bytes.take(32) =~= entries[0].0.0@);
        hash_at_unique(entries[0].0,bytes);
        assert forall|i:int| 0 <= i < tail.len() implies target.contains_key(tail[i].0) && target[tail[i].0] == tail[i].1 && !next.contains_key(tail[i].0) by {
            assert(entries.map_values(|e:(CommitHash,bool)| e.0)[i+1] != entries.map_values(|e:(CommitHash,bool)| e.0)[0]);
        }
        assert forall|k:CommitHash| target.contains_key(k) == (next.contains_key(k) || tail.map_values(|e:(CommitHash,bool)| e.0).contains(k)) by {
            if target.contains_key(k) && !next.contains_key(k) {
                let j = choose|j:int| 0 <= j < entries.len() && entries[j].0 == k;
                assert(j > 0);
                assert(tail.map_values(|e:(CommitHash,bool)| e.0)[j-1] == k);
            }
        }
        assert(tail.map_values(|e:(CommitHash,bool)| e.0) =~= entries.map_values(|e:(CommitHash,bool)| e.0).skip(1));
        assert(next.submap_of(target));
        entries_roundtrip(tail,target,next,suffix);
        assert(bytes.skip(33) =~= entries_bytes(tail)+suffix);
    }
}
pub(crate) proof fn encoding_decodes(map:Map<CommitHash,bool>,bytes:Seq<u8>,suffix:Seq<u8>)
    requires encoded(map,bytes), (bytes+suffix).len() <= usize::MAX,
    ensures parse(bytes+suffix) == Some((map,suffix)),
{
    reveal(parse);
    let entries = choose|entries:Seq<(CommitHash,bool)>| represents(entries,map) && sorted(entries) && entries.len() <= usize::MAX
        && bytes == vstd::bytes::spec_u64_to_le_bytes(entries.len() as u64)+entries_bytes(entries);
    entries_size(entries);
    super::metadata_encoding::u64_roundtrip(entries.len() as u64);
    let input = bytes+suffix;
    assert(super::reader::fits_count(input,33)) by(nonlinear_arith)
        requires input.len() >= 8+33*entries.len(), super::reader::prefix_count(input) == entries.len(), entries.len() <= usize::MAX;
    entries.to_set_ensures();
    assert forall|i:int| 0 <= i < entries.len() implies map.contains_key(entries[i].0) && map[entries[i].0] == entries[i].1 by {
        assert(map.kv_pairs().contains(entries[i]));
    }
    assert forall|k:CommitHash| map.contains_key(k) == entries.map_values(|e:(CommitHash,bool)| e.0).contains(k) by {
        if map.contains_key(k) {
            assert(entries.to_set().contains((k,map[k])));
            let i = choose|i:int| 0 <= i < entries.len() && entries[i] == (k,map[k]);
            assert(entries.map_values(|e:(CommitHash,bool)| e.0)[i] == k);
        } else if entries.map_values(|e:(CommitHash,bool)| e.0).contains(k) {
            let i = choose|i:int| 0 <= i < entries.len() && entries[i].0 == k;
        }
    }
    assert(remaining(entries,map,Map::empty()));
    entries_roundtrip(entries,map,Map::empty(),suffix);
    assert(input.skip(8) =~= entries_bytes(entries)+suffix);
}
} // verus!
