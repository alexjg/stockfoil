//! Checked view and public materialization-cursor serialization.
use super::{
    api_error::Error,
    metadata_encoding::encode_hashes,
    reader::Reader,
    view::{MaterializationCursor, ViewPoint},
};
#[cfg(verus_keep_ghost)]
use super::{
    author::Author,
    commit_hash::CommitHash,
    metadata_encoding::{encoded_hashes, hash_layout},
    permission_codec::permission_bytes,
    permission_map_codec as maps,
    reader::{fits_count, prefix_count},
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {
broadcast use {vstd::seq::group_seq_lemmas, vstd::seq_lib::group_seq_properties};

#[cfg(verus_keep_ghost)]
pub type ViewData = (Seq<CommitHash>, Map<Author,Seq<u8>>, Seq<u8>);
#[cfg(verus_keep_ghost)]
pub type CursorData = (Seq<CommitHash>, ViewData);

#[verifier::opaque]
pub(crate) open spec fn hashes_data(bytes: Seq<u8>) -> Seq<CommitHash> {
    Seq::new(prefix_count(bytes) as nat, |i: int| CommitHash(maps::author_at(bytes.subrange(8+32*i,8+32*(i+1))).0))
}
pub(crate) open spec fn hashes_end(bytes: Seq<u8>) -> int { 8+32*prefix_count(bytes) }
pub(crate) proof fn hashes_match(hashes: Seq<CommitHash>, bytes: Seq<u8>)
    requires fits_count(bytes,32), hashes.len() == prefix_count(bytes),
        forall|i: int| 0 <= i < hashes.len() ==> hashes[i].0@ == bytes.subrange(8+32*i,8+32*(i+1)),
    ensures hashes_data(bytes) == hashes,
{
    reveal(hashes_data);
    assert forall|i: int| 0 <= i < hashes.len() implies hashes_data(bytes)[i] == hashes[i] by {
        maps::author_at_unique(Author(hashes[i].0),bytes.subrange(8+32*i,8+32*(i+1)));
    }
    assert(hashes_data(bytes) =~= hashes);
}
pub(crate) proof fn hashes_layout(hashes: Seq<CommitHash>, suffix: Seq<u8>)
    requires hashes.len() <= usize::MAX, (encoded_hashes(hashes)+suffix).len() <= usize::MAX,
    ensures fits_count(encoded_hashes(hashes)+suffix,32), hashes_data(encoded_hashes(hashes)+suffix) == hashes,
        (encoded_hashes(hashes)+suffix).skip(hashes_end(encoded_hashes(hashes)+suffix)) == suffix,
{
    hash_layout(hashes);
    let bytes = encoded_hashes(hashes)+suffix;
    assert(fits_count(bytes,32)) by(nonlinear_arith)
        requires bytes.len() == 8+32*hashes.len()+suffix.len(), prefix_count(bytes) == hashes.len(), hashes.len() <= usize::MAX;
    assert forall|i: int| 0 <= i < hashes.len() implies hashes[i].0@ == bytes.subrange(8+32*i,8+32*(i+1)) by {
        assert(encoded_hashes(hashes).subrange(8+32*i,8+32*(i+1)) == hashes[i].0@);
    }
    hashes_match(hashes,bytes);
    assert(bytes.skip(hashes_end(bytes)) =~= suffix);
}

pub(crate) open spec fn view_data(view: &ViewPoint) -> ViewData {
    (view.heads@,maps::values(&view.write_permissions),permission_bytes(&view.write_permissions.fallback()))
}
pub(crate) open spec fn view_encoded(view: &ViewPoint, bytes: Seq<u8>) -> bool {
    exists|tail: Seq<u8>| maps::encoded(&view.write_permissions,tail) && bytes == encoded_hashes(view.heads@)+tail
}
#[verifier::opaque]
pub(crate) open spec fn view_parse(bytes: Seq<u8>) -> Option<(ViewData,Seq<u8>)> {
    if !fits_count(bytes,32) { None }
    else { match maps::parse(bytes.skip(hashes_end(bytes))) {
        None => None,
        Some((map,fallback,rest)) => Some(((hashes_data(bytes),map,fallback),rest)),
    } }
}
pub(crate) proof fn view_roundtrip(view: &ViewPoint, bytes: Seq<u8>, suffix: Seq<u8>)
    requires view_encoded(view,bytes), (bytes+suffix).len() <= usize::MAX,
    ensures view_parse(bytes+suffix) == Some((view_data(view),suffix)),
{
    reveal(view_parse);
    let tail = choose|tail: Seq<u8>| maps::encoded(&view.write_permissions,tail) && bytes == encoded_hashes(view.heads@)+tail;
    vstd::std_specs::vec::axiom_spec_len(&view.heads);
    assert(bytes+suffix =~= encoded_hashes(view.heads@)+(tail+suffix));
    hashes_layout(view.heads@,tail+suffix);
    maps::encoding_decodes(&view.write_permissions,tail,suffix);
}

pub(crate) proof fn view_data_same(a:&ViewPoint,b:&ViewPoint)
    ensures (view_data(a) == view_data(b)) == a.same(b),
{
    a.representation(); b.representation();
    maps::same_values(&a.write_permissions,&b.write_permissions);
}

pub(crate) proof fn view_encoding_unique(a: &ViewPoint, b: &ViewPoint, first: Seq<u8>, second: Seq<u8>)
    requires a.same(b), view_encoded(a,first), view_encoded(b,second),
    ensures first == second,
{
    a.representation(); b.representation();
    let ta = choose|tail: Seq<u8>| maps::encoded(&a.write_permissions,tail) && first == encoded_hashes(a.heads@)+tail;
    let tb = choose|tail: Seq<u8>| maps::encoded(&b.write_permissions,tail) && second == encoded_hashes(b.heads@)+tail;
    maps::encoding_unique(&a.write_permissions,&b.write_permissions,ta,tb);
}

impl ViewPoint {
    pub(crate) fn encode_into(&self, out: &mut Vec<u8>)
        ensures exists|bytes: Seq<u8>| view_encoded(self,bytes) && final(out)@ == old(out)@+bytes,
    {
        let ghost prefix = out@;
        encode_hashes(out,&self.heads);
        let ghost before = out@;
        super::permission_map_codec::encode(out,&self.write_permissions);
        proof {
            let tail = choose|bytes: Seq<u8>| maps::encoded(&self.write_permissions,bytes) && out@ == before+bytes;
            let bytes = encoded_hashes(self.heads@)+tail;
            assert(view_encoded(self,bytes));
            assert(out@ == prefix+bytes);
        }
    }

    pub(crate) fn decode_from(r: &mut Reader<'_>) -> (out: Result<Self,Error>)
        ensures match out {
            Ok(view) => view_parse(old(r).unread()) == Some((view_data(&view),final(r).unread())),
            Err(e) => e is Malformed && view_parse(old(r).unread()) is None,
        },
    {
        proof { reveal(view_parse); }
        let ghost bytes = r.unread();
        let heads = r.hashes()?;
        proof { hashes_match(heads@,bytes); }
        let write_permissions = super::permission_map_codec::decode(r)?;
        Ok(Self { heads,write_permissions })
    }
}

pub(crate) open spec fn magic() -> Seq<u8> { seq![83u8,70u8,77u8,67u8,1u8] }

impl MaterializationCursor {
    pub closed spec fn encoding_matches(&self, bytes: Seq<u8>) -> bool {
        exists|tail: Seq<u8>| view_encoded(&self.view_point,tail)
            && bytes == magic()+encoded_hashes(self.delivery_heads@)+tail
    }
    pub proof fn encoding_deterministic(&self, other: &Self, first: Seq<u8>, second: Seq<u8>)
        requires self.same(other), self.encoding_matches(first), other.encoding_matches(second),
        ensures first == second,
    {
        self.representation(); other.representation();
        let a = choose|tail: Seq<u8>| view_encoded(&self.view_point,tail) && first == magic()+encoded_hashes(self.delivery_heads@)+tail;
        let b = choose|tail: Seq<u8>| view_encoded(&other.view_point,tail) && second == magic()+encoded_hashes(other.delivery_heads@)+tail;
        view_encoding_unique(&self.view_point,&other.view_point,a,b);
    }
    pub closed spec fn codec_data(&self) -> CursorData { (self.delivery_heads@,view_data(&self.view_point)) }
    pub closed spec fn decode_model(bytes: Seq<u8>) -> Option<CursorData> {
        if bytes.len() < 5 || bytes.take(5) != magic() { None }
        else {
            let body = bytes.skip(5);
            if !fits_count(body,32) { None }
            else { match view_parse(body.skip(hashes_end(body))) {
                Some((view,rest)) => if rest.len() == 0 { Some((hashes_data(body),view)) } else { None },
                None => None,
            } }
        }
    }
    pub proof fn same_codec_data(&self, other: &Self)
        ensures (self.codec_data() == other.codec_data()) == self.same(other),
    {
        self.representation(); other.representation();
        self.view_point.representation(); other.view_point.representation();
        maps::same_values(&self.view_point.write_permissions,&other.view_point.write_permissions);
    }

    /// Deterministic, versioned encoding of completed delivery and visibility progress.
    pub fn to_bytes(&self) -> (out: Vec<u8>)
        ensures Self::decode_model(out@) == Some(self.codec_data()), self.encoding_matches(out@),
    {
        let mut out = vec![83u8,70u8,77u8,67u8,1u8];
        encode_hashes(&mut out,&self.delivery_heads);
        let ghost before = out@;
        self.view_point.encode_into(&mut out);
        proof {
            let bytes = choose|bytes: Seq<u8>| view_encoded(&self.view_point,bytes) && out@ == before+bytes;
            vstd::std_specs::vec::axiom_spec_len(&out);
            vstd::std_specs::vec::axiom_spec_len(&self.delivery_heads);
            assert(self.encoding_matches(out@));
            view_roundtrip(&self.view_point,bytes,Seq::empty());
            hashes_layout(self.delivery_heads@,bytes);
            assert(out@.take(5) =~= magic());
            assert(out@.skip(5) =~= encoded_hashes(self.delivery_heads@)+bytes);
        }
        out
    }

    /// Reject malformed collections, duplicate authors, unknown versions/tags and trailing bytes.
    pub fn from_bytes(bytes: &[u8]) -> (out: Result<Self,Error>)
        ensures match out {
            Ok(cursor) => Self::decode_model(bytes@) == Some(cursor.codec_data()),
            Err(e) => e is Malformed && Self::decode_model(bytes@) is None,
        },
    {
        match Self::decode_cursor(bytes) {
            Ok(cursor) => Ok(cursor),
            Err(_) => Err(Error::Malformed("materialization cursor")),
        }
    }

    fn decode_cursor(bytes: &[u8]) -> (out: Result<Self,Error>)
        ensures match out {
            Ok(cursor) => Self::decode_model(bytes@) == Some(cursor.codec_data()),
            Err(e) => e is Malformed && Self::decode_model(bytes@) is None,
        },
    {
        let mut r = Reader::new(bytes);
        let header = r.array::<5>()?;
        if header != [83u8,70u8,77u8,67u8,1u8] { return Err(Error::Malformed("materialization cursor")); }
        proof { assert(header@ =~= magic()); }
        let delivery_heads = r.hashes()?;
        proof { hashes_match(delivery_heads@,bytes@.skip(5)); }
        let ghost tail = r.unread();
        let view_point = ViewPoint::decode_from(&mut r)?;
        r.finish()?;
        Ok(Self { delivery_heads,view_point })
    }
}
} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Author, CommitHash, WritePermission, WritePermissions};

    // Independent copy of the prior format for compatibility, using native conversions.
    fn legacy(cursor: &MaterializationCursor) -> Vec<u8> {
        fn hashes(out: &mut Vec<u8>, values: &[CommitHash]) {
            out.extend((values.len() as u64).to_le_bytes());
            for hash in values {
                out.extend(hash.0);
            }
        }
        fn permission(out: &mut Vec<u8>, p: &WritePermission) {
            match p {
                WritePermission::Unbounded => out.push(0),
                WritePermission::Denied => out.push(1),
                WritePermission::Bounded(heads) => {
                    out.push(2);
                    hashes(out, heads);
                }
            }
        }
        let mut out = b"SFMC\x01".to_vec();
        hashes(&mut out, &cursor.delivery_heads);
        hashes(&mut out, &cursor.view_point.heads);
        let mut entries: Vec<_> = cursor.view_point.write_permissions.iter().collect();
        entries.sort_unstable_by_key(|(author, _)| **author);
        out.extend((entries.len() as u64).to_le_bytes());
        for (author, p) in entries {
            out.extend(author.0);
            permission(&mut out, p);
        }
        permission(&mut out, cursor.view_point.write_permissions.unlisted());
        out
    }

    #[test]
    fn public_cursor_encoding_preserves_the_existing_format() {
        let repeated = vec![
            CommitHash([7; 32]),
            CommitHash([3; 32]),
            CommitHash([7; 32]),
        ];
        for count in 0..16 {
            for fallback in [
                WritePermission::Denied,
                WritePermission::Unbounded,
                WritePermission::Bounded(repeated.clone()),
            ] {
                let mut p = WritePermissions::new().with_unlisted(fallback);
                for i in (0..count).rev() {
                    p.insert(
                        Author([i as u8; 32]),
                        match i % 3 {
                            0 => WritePermission::Unbounded,
                            1 => WritePermission::Denied,
                            _ => WritePermission::Bounded(repeated.clone()),
                        },
                    );
                }
                let cursor = MaterializationCursor::from_parts(
                    repeated.clone(),
                    ViewPoint::from_parts(repeated.clone(), p),
                );
                let bytes = cursor.to_bytes();
                assert_eq!(bytes, legacy(&cursor));
                assert_eq!(MaterializationCursor::from_bytes(&bytes).unwrap(), cursor);
            }
        }
    }

    #[test]
    fn public_cursor_accepts_unique_unsorted_authors_and_rejects_malformed_data() {
        let p = WritePermissions::from([
            (Author([1; 32]), WritePermission::Denied),
            (Author([2; 32]), WritePermission::Unbounded),
        ]);
        let cursor = MaterializationCursor::from_parts(vec![], ViewPoint::from_parts(vec![], p));
        let bytes = cursor.to_bytes();
        let mut reversed = bytes.clone();
        reversed[29..62].copy_from_slice(&bytes[62..95]);
        reversed[62..95].copy_from_slice(&bytes[29..62]);
        let decoded = MaterializationCursor::from_bytes(&reversed).unwrap();
        assert_eq!(decoded, cursor);
        assert_eq!(decoded.to_bytes(), bytes);
        let mut duplicate = bytes.clone();
        duplicate[62..94].copy_from_slice(&bytes[29..61]);
        assert!(MaterializationCursor::from_bytes(&duplicate).is_err());
        for (offset, value) in [(0, 0), (4, 2), (61, 3), (95, 3)] {
            let mut malformed = bytes.clone();
            malformed[offset] = value;
            assert!(MaterializationCursor::from_bytes(&malformed).is_err());
        }
        for len in 0..bytes.len() {
            assert!(MaterializationCursor::from_bytes(&bytes[..len]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(MaterializationCursor::from_bytes(&trailing).is_err());
    }
}
