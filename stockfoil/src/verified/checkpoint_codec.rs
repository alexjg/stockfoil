//! Public checkpoint serialization: exact metadata round trips and structural parsing.
use super::{
    api_error::Error, checkpoint::Checkpoint, checkpoint_parts as parts,
    metadata_encoding::encode_u64, reader::Reader, rejection_codec as rejections,
    view::MaterializationCursor,
};
#[cfg(verus_keep_ghost)]
use super::{
    checkpoint_parts::{FollowData, ProgressData},
    commit_hash::CommitHash,
    reader::{fits_count, prefix_count},
    view_codec::CursorData,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {
broadcast use {vstd::seq::group_seq_lemmas,vstd::seq_lib::group_seq_properties};
#[cfg(verus_keep_ghost)]
pub type CheckpointData = (FollowData,CursorData,Map<CommitHash,bool>,ProgressData);

pub(crate) open spec fn framed(cursor:&MaterializationCursor,bytes:Seq<u8>) -> bool {
    exists|body:Seq<u8>| body.len() <= usize::MAX && cursor.encoding_matches(body)
        && MaterializationCursor::decode_model(body) == Some(cursor.codec_data())
        && bytes == vstd::bytes::spec_u64_to_le_bytes(body.len() as u64)+body
}
#[verifier::opaque]
pub(crate) open spec fn parse_frame(bytes:Seq<u8>) -> Option<(CursorData,Seq<u8>)> {
    if !fits_count(bytes,1) { None }
    else { match MaterializationCursor::decode_model(bytes.subrange(8,8+prefix_count(bytes))) {
        None => None, Some(cursor) => Some((cursor,bytes.skip(8+prefix_count(bytes)))),
    } }
}
pub(crate) proof fn frame_roundtrip(cursor:&MaterializationCursor,bytes:Seq<u8>,suffix:Seq<u8>)
    requires framed(cursor,bytes), (bytes+suffix).len() <= usize::MAX,
    ensures parse_frame(bytes+suffix) == Some((cursor.codec_data(),suffix)),
{
    reveal(parse_frame);
    let body = choose|body:Seq<u8>| body.len() <= usize::MAX && cursor.encoding_matches(body)
        && MaterializationCursor::decode_model(body) == Some(cursor.codec_data())
        && bytes == vstd::bytes::spec_u64_to_le_bytes(body.len() as u64)+body;
    super::metadata_encoding::u64_roundtrip(body.len() as u64);
    assert((bytes+suffix).subrange(8,8+body.len() as int) =~= body);
    assert((bytes+suffix).skip(8+body.len() as int) =~= suffix);
}
pub(crate) proof fn frame_unique(a:&MaterializationCursor,b:&MaterializationCursor,x:Seq<u8>,y:Seq<u8>)
    requires a.same(b),framed(a,x),framed(b,y),
    ensures x == y,
{
    let bx = choose|body:Seq<u8>| body.len() <= usize::MAX && a.encoding_matches(body)
        && MaterializationCursor::decode_model(body) == Some(a.codec_data())
        && x == vstd::bytes::spec_u64_to_le_bytes(body.len() as u64)+body;
    let by = choose|body:Seq<u8>| body.len() <= usize::MAX && b.encoding_matches(body)
        && MaterializationCursor::decode_model(body) == Some(b.codec_data())
        && y == vstd::bytes::spec_u64_to_le_bytes(body.len() as u64)+body;
    a.encoding_deterministic(b,bx,by);
}
fn encode_frame(out:&mut Vec<u8>,cursor:&MaterializationCursor)
    ensures exists|bytes:Seq<u8>| framed(cursor,bytes) && final(out)@ == old(out)@+bytes,
{
    let ghost prefix = out@;
    let body = cursor.to_bytes();
    encode_u64(out,body.len() as u64);
    out.extend_from_slice(&body);
    proof {
        let bytes = vstd::bytes::spec_u64_to_le_bytes(body.len() as u64)+body@;
        assert(framed(cursor,bytes));
        assert(out@ == prefix+bytes);
    }
}
fn decode_frame(r:&mut Reader<'_>) -> (out:Result<MaterializationCursor,Error>)
    ensures match out {
        Ok(cursor) => parse_frame(old(r).unread()) == Some((cursor.codec_data(),final(r).unread())),
        Err(e) => e is Malformed && parse_frame(old(r).unread()) is None,
    },
{
    proof { reveal(parse_frame); }
    let ghost original = r.unread();
    let len = r.count(1)?;
    let body = r.take(len)?;
    assert(body@ =~= original.subrange(8,8+len));
    assert(r.unread() =~= original.skip(8+len));
    MaterializationCursor::from_bytes(body)
}

pub(crate) open spec fn magic() -> Seq<u8> { seq![83u8,70u8,67u8,80u8,1u8] }
impl Checkpoint {
    pub closed spec fn codec_data(&self) -> CheckpointData {
        (parts::follow_data(&self.follow),self.cursor.codec_data(),self.rejected@,parts::progress_data(&self.in_progress))
    }
    pub closed spec fn decode_model(bytes:Seq<u8>) -> Option<CheckpointData> {
        if bytes.len() < 5 || bytes.take(5) != magic() { None }
        else { match parts::parse_follow(bytes.skip(5)) {
            None => None, Some((follow,a)) => match parse_frame(a) {
                None => None, Some((cursor,b)) => match rejections::parse(b) {
                    None => None, Some((rejected,c)) => match parts::parse_progress(c) {
                        None => None, Some((progress,rest)) => if rest.len() == 0 { Some((follow,cursor,rejected,progress)) } else { None },
                    },
                },
            },
        } }
    }
    pub closed spec fn encoding_matches(&self,bytes:Seq<u8>) -> bool {
        exists|cursor:Seq<u8>,rejected:Seq<u8>,progress:Seq<u8>| framed(&self.cursor,cursor)
            && rejections::encoded(self.rejected@,rejected) && parts::progress_encoded(&self.in_progress,progress)
            && bytes == magic()+parts::follow_bytes(&self.follow)+cursor+rejected+progress
    }
    pub proof fn same_codec_data(&self,other:&Self)
        ensures (self.codec_data() == other.codec_data()) == self.same(other),
    {
        self.representation(); other.representation();
        parts::follow_same(&self.follow,&other.follow);
        self.cursor.same_codec_data(&other.cursor);
        parts::progress_same(&self.in_progress,&other.in_progress);
    }
    pub proof fn encoding_deterministic(&self,other:&Self,first:Seq<u8>,second:Seq<u8>)
        requires self.same(other), self.encoding_matches(first), other.encoding_matches(second),
        ensures first == second,
    {
        self.representation(); other.representation();
        let (c1,r1,p1) = choose|c:Seq<u8>,r:Seq<u8>,p:Seq<u8>| framed(&self.cursor,c)
            && rejections::encoded(self.rejected@,r) && parts::progress_encoded(&self.in_progress,p)
            && first == magic()+parts::follow_bytes(&self.follow)+c+r+p;
        let (c2,r2,p2) = choose|c:Seq<u8>,r:Seq<u8>,p:Seq<u8>| framed(&other.cursor,c)
            && rejections::encoded(other.rejected@,r) && parts::progress_encoded(&other.in_progress,p)
            && second == magic()+parts::follow_bytes(&other.follow)+c+r+p;
        parts::follow_same(&self.follow,&other.follow);
        frame_unique(&self.cursor,&other.cursor,c1,c2);
        rejections::encoding_unique(self.rejected@,r1,r2);
        parts::progress_unique(&self.in_progress,&other.in_progress,p1,p2);
    }

    /// A deterministic, versioned encoding. Permission maps and rejected sets
    /// are encoded in sorted key order, independent of map insertion order.
    pub fn to_bytes(&self) -> (out:Vec<u8>)
        ensures Self::decode_model(out@) == Some(self.codec_data()), self.encoding_matches(out@),
    {
        let mut out = vec![83u8,70u8,67u8,80u8,1u8];
        parts::encode_follow(&mut out,&self.follow);
        let ghost before_cursor = out@;
        encode_frame(&mut out,&self.cursor);
        let ghost cursor = choose|bytes:Seq<u8>| framed(&self.cursor,bytes) && out@ == before_cursor+bytes;
        let ghost before_rejected = out@;
        rejections::encode(&mut out,&self.rejected);
        let ghost rejected = choose|bytes:Seq<u8>| rejections::encoded(self.rejected@,bytes) && out@ == before_rejected+bytes;
        let ghost before_progress = out@;
        parts::encode_progress(&mut out,&self.in_progress);
        proof {
            let progress = choose|bytes:Seq<u8>| parts::progress_encoded(&self.in_progress,bytes) && out@ == before_progress+bytes;
            vstd::std_specs::vec::axiom_spec_len(&out);
            assert(out@ == magic()+parts::follow_bytes(&self.follow)+cursor+rejected+progress);
            assert(self.encoding_matches(out@));
            parts::progress_roundtrip(&self.in_progress,progress,Seq::empty());
            rejections::encoding_decodes(self.rejected@,rejected,progress);
            frame_roundtrip(&self.cursor,cursor,rejected+progress);
            parts::follow_roundtrip(&self.follow,cursor+(rejected+progress));
            assert(out@.take(5) =~= magic());
            assert(out@.skip(5) =~= parts::follow_bytes(&self.follow)+(cursor+(rejected+progress)));
        }
        out
    }

    /// Decode a checkpoint without consulting a history. Invalid/truncated
    /// collections, duplicate keys, trailing bytes, and unknown versions or
    /// tags are errors. Semantic validation happens when the planner resumes.
    pub fn from_bytes(bytes:&[u8]) -> (out:Result<Self,Error>)
        ensures match out {
            Ok(checkpoint) => Self::decode_model(bytes@) == Some(checkpoint.codec_data()),
            Err(e) => e is Malformed && Self::decode_model(bytes@) is None,
        },
    {
        match Self::decode(bytes) { Ok(c) => Ok(c), Err(_) => Err(Error::Malformed("materialization checkpoint")) }
    }
    fn decode(bytes:&[u8]) -> (out:Result<Self,Error>)
        ensures match out {
            Ok(checkpoint) => Self::decode_model(bytes@) == Some(checkpoint.codec_data()),
            Err(e) => e is Malformed && Self::decode_model(bytes@) is None,
        },
    {
        let mut r = Reader::new(bytes);
        let header = r.array::<5>()?;
        if header != [83u8,70u8,67u8,80u8,1u8] { return Err(Error::Malformed("checkpoint version")); }
        proof { assert(header@ =~= magic()); }
        let follow = parts::decode_follow(&mut r)?;
        let cursor = decode_frame(&mut r)?;
        let rejected = rejections::decode(&mut r)?;
        let in_progress = parts::decode_progress(&mut r)?;
        r.finish()?;
        Ok(Self { follow,cursor,rejected,in_progress })
    }
}
} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verified::{checkpoint::InProgress, view::ViewPoint};
    use crate::{Author, CommitHash, Follow, WritePermission, WritePermissions};
    use std::collections::BTreeMap;

    // Independent copy of the previous outer format, with native length/boolean conversions.
    fn legacy(checkpoint: &Checkpoint) -> Vec<u8> {
        fn hashes(out: &mut Vec<u8>, heads: &[CommitHash]) {
            out.extend((heads.len() as u64).to_le_bytes());
            for h in heads {
                out.extend(h.0);
            }
        }
        let mut out = b"SFCP\x01".to_vec();
        match &checkpoint.follow {
            Follow::Working => out.push(0),
            Follow::Signed => out.push(1),
            Follow::Pinned(heads) => {
                out.push(2);
                hashes(&mut out, heads);
            }
        }
        let cursor = checkpoint.cursor.to_bytes();
        out.extend((cursor.len() as u64).to_le_bytes());
        out.extend(cursor);
        out.extend((checkpoint.rejected.len() as u64).to_le_bytes());
        for (h, cascade) in &checkpoint.rejected {
            out.extend(h.0);
            out.push(u8::from(*cascade));
        }
        match &checkpoint.in_progress {
            None => out.push(0),
            Some(p) => {
                out.push(1);
                p.target.encode_into(&mut out);
                hashes(&mut out, &p.delivery_heads);
                out.extend(p.applied.to_le_bytes());
            }
        }
        out
    }

    #[test]
    fn checkpoint_format_is_compatible_for_all_follow_and_progress_variants() {
        let heads = vec![
            CommitHash([7; 32]),
            CommitHash([3; 32]),
            CommitHash([7; 32]),
        ];
        let mut permissions =
            WritePermissions::new().with_unlisted(WritePermission::Bounded(heads.clone()));
        permissions.insert(Author([2; 32]), WritePermission::Denied);
        permissions.insert(Author([1; 32]), WritePermission::Unbounded);
        let target = ViewPoint::from_parts(heads.clone(), permissions);
        for follow in [
            Follow::Working,
            Follow::Signed,
            Follow::Pinned(vec![]),
            Follow::Pinned(heads.clone()),
        ] {
            for applied in [None, Some(0), Some(1), Some(u64::MAX)] {
                for rejected in [
                    BTreeMap::new(),
                    BTreeMap::from([(heads[0], false), (heads[1], true)]),
                ] {
                    let checkpoint = Checkpoint {
                        follow: follow.clone(),
                        cursor: MaterializationCursor::from_parts(heads.clone(), target.clone()),
                        rejected,
                        in_progress: applied.map(|applied| InProgress {
                            target: target.clone(),
                            delivery_heads: heads.clone(),
                            applied,
                        }),
                    };
                    let bytes = checkpoint.to_bytes();
                    assert_eq!(bytes, legacy(&checkpoint));
                    assert_eq!(Checkpoint::from_bytes(&bytes).unwrap(), checkpoint);
                }
            }
        }
    }

    #[test]
    fn checkpoint_rejection_entries_accept_unsorted_keys_and_reject_duplicates_and_bad_flags() {
        let mut checkpoint = Checkpoint::new(Follow::Working);
        checkpoint.rejected =
            BTreeMap::from([(CommitHash([1; 32]), false), (CommitHash([2; 32]), true)]);
        let bytes = checkpoint.to_bytes();
        let start = 5 + 1 + 8 + checkpoint.cursor.to_bytes().len() + 8;
        let mut unsorted = bytes.clone();
        unsorted[start..start + 33].copy_from_slice(&bytes[start + 33..start + 66]);
        unsorted[start + 33..start + 66].copy_from_slice(&bytes[start..start + 33]);
        let decoded = Checkpoint::from_bytes(&unsorted).unwrap();
        assert_eq!(decoded, checkpoint);
        assert_eq!(decoded.to_bytes(), bytes);
        let mut duplicate = bytes.clone();
        duplicate[start + 33..start + 65].copy_from_slice(&bytes[start..start + 32]);
        assert!(Checkpoint::from_bytes(&duplicate).is_err());
        for offset in [start + 32, start + 65, start + 66] {
            let mut malformed = bytes.clone();
            malformed[offset] = 2;
            assert!(Checkpoint::from_bytes(&malformed).is_err());
        }
        for len in 0..bytes.len() {
            assert!(Checkpoint::from_bytes(&bytes[..len]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(Checkpoint::from_bytes(&trailing).is_err());
    }
}
