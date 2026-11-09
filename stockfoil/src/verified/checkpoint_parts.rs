//! Checked follow rules and optional partial-traversal checkpoints.
use super::{
    api_error::Error,
    checkpoint::{Follow, InProgress},
    metadata_encoding::{encode_hashes, encode_u64},
    permission_codec::read_bool,
    reader::Reader,
    view::ViewPoint,
};
#[cfg(verus_keep_ghost)]
use super::{
    commit_hash::CommitHash,
    metadata_encoding::encoded_hashes,
    reader::{fits_count, prefix_count},
    view_codec::{self, ViewData},
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {
broadcast use {vstd::seq::group_seq_lemmas,vstd::seq_lib::group_seq_properties};
#[cfg(verus_keep_ghost)]
pub type FollowData = (u8,Seq<CommitHash>);
#[cfg(verus_keep_ghost)]
pub type ProgressData = Option<(ViewData,Seq<CommitHash>,u64)>;

pub(crate) open spec fn follow_data(f:&Follow) -> FollowData {
    match f { Follow::Working => (0,Seq::empty()), Follow::Signed => (1,Seq::empty()), Follow::Pinned(heads) => (2,heads@) }
}
pub(crate) open spec fn follow_bytes(f:&Follow) -> Seq<u8> {
    match f { Follow::Working => seq![0u8], Follow::Signed => seq![1u8], Follow::Pinned(heads) => seq![2u8]+encoded_hashes(heads@) }
}
#[verifier::opaque]
pub(crate) open spec fn parse_follow(bytes:Seq<u8>) -> Option<(FollowData,Seq<u8>)> {
    if bytes.len() == 0 { None }
    else if bytes[0] < 2 { Some(((bytes[0],Seq::empty()),bytes.skip(1))) }
    else if bytes[0] == 2 && fits_count(bytes.skip(1),32) {
        Some(((2,view_codec::hashes_data(bytes.skip(1))),bytes.skip(1).skip(view_codec::hashes_end(bytes.skip(1)))))
    } else { None }
}
pub(crate) proof fn follow_same(a:&Follow,b:&Follow)
    ensures (follow_data(a) == follow_data(b)) == a.same(b), a.same(b) ==> follow_bytes(a) == follow_bytes(b),
{}
pub(crate) proof fn follow_roundtrip(f:&Follow,suffix:Seq<u8>)
    requires (follow_bytes(f)+suffix).len() <= usize::MAX,
    ensures parse_follow(follow_bytes(f)+suffix) == Some((follow_data(f),suffix)),
{
    reveal(parse_follow);
    if let Follow::Pinned(heads) = f {
        vstd::std_specs::vec::axiom_spec_len(heads);
        view_codec::hashes_layout(heads@,suffix);
        assert((follow_bytes(f)+suffix).skip(1) =~= encoded_hashes(heads@)+suffix);
    }
}
pub(crate) fn encode_follow(out:&mut Vec<u8>,f:&Follow)
    ensures final(out)@ == old(out)@+follow_bytes(f),
{
    match f { Follow::Working => out.push(0), Follow::Signed => out.push(1),
        Follow::Pinned(heads) => { out.push(2); encode_hashes(out,heads); }, }
}
pub(crate) fn decode_follow(r:&mut Reader<'_>) -> (out:Result<Follow,Error>)
    ensures match out {
        Ok(f) => parse_follow(old(r).unread()) == Some((follow_data(&f),final(r).unread())),
        Err(e) => e is Malformed && parse_follow(old(r).unread()) is None,
    },
{
    proof { reveal(parse_follow); }
    let tag = r.take(1)?[0];
    let ghost body = r.unread();
    Ok(match tag {
        0 => Follow::Working, 1 => Follow::Signed,
        2 => { let heads = r.hashes()?; proof { view_codec::hashes_match(heads@,body); } Follow::Pinned(heads) },
        _ => return Err(Error::Malformed("checkpoint follow rule")),
    })
}

pub(crate) open spec fn progress_data(p:&Option<InProgress>) -> ProgressData {
    match p { None => None, Some(p) => Some((view_codec::view_data(&p.target),p.delivery_heads@,p.applied)) }
}
pub(crate) open spec fn progress_encoded(p:&Option<InProgress>,bytes:Seq<u8>) -> bool {
    match p {
        None => bytes == seq![0u8],
        Some(p) => exists|view:Seq<u8>| view_codec::view_encoded(&p.target,view)
            && bytes == seq![1u8]+view+encoded_hashes(p.delivery_heads@)+vstd::bytes::spec_u64_to_le_bytes(p.applied),
    }
}
#[verifier::opaque]
pub(crate) open spec fn parse_progress(bytes:Seq<u8>) -> Option<(ProgressData,Seq<u8>)> {
    if bytes.len() == 0 { None }
    else if bytes[0] == 0 { Some((None,bytes.skip(1))) }
    else if bytes[0] != 1 { None }
    else { match view_codec::view_parse(bytes.skip(1)) {
        None => None,
        Some((target,tail)) => if !fits_count(tail,32) { None } else {
            let end = tail.skip(view_codec::hashes_end(tail));
            if end.len() < 8 { None } else { Some((Some((target,view_codec::hashes_data(tail),prefix_count(end))),end.skip(8))) }
        },
    } }
}
pub(crate) proof fn progress_same(a:&Option<InProgress>,b:&Option<InProgress>)
    ensures (progress_data(a) == progress_data(b)) == super::checkpoint::same_progress(*a,*b),
{
    if let (Some(x),Some(y)) = (a,b) { view_codec::view_data_same(&x.target,&y.target); }
}
pub(crate) proof fn progress_roundtrip(p:&Option<InProgress>,bytes:Seq<u8>,suffix:Seq<u8>)
    requires progress_encoded(p,bytes), (bytes+suffix).len() <= usize::MAX,
    ensures parse_progress(bytes+suffix) == Some((progress_data(p),suffix)),
{
    reveal(parse_progress);
    if let Some(p) = p {
        let view = choose|view:Seq<u8>| view_codec::view_encoded(&p.target,view)
            && bytes == seq![1u8]+view+encoded_hashes(p.delivery_heads@)+vstd::bytes::spec_u64_to_le_bytes(p.applied);
        vstd::std_specs::vec::axiom_spec_len(&p.delivery_heads);
        super::metadata_encoding::u64_roundtrip(p.applied);
        let word = vstd::bytes::spec_u64_to_le_bytes(p.applied);
        let tail = encoded_hashes(p.delivery_heads@)+(word+suffix);
        view_codec::view_roundtrip(&p.target,view,tail);
        view_codec::hashes_layout(p.delivery_heads@,word+suffix);
        assert((bytes+suffix).skip(1) =~= view+tail);
        assert((word+suffix).skip(8) =~= suffix);
    }
}
pub(crate) proof fn progress_unique(a:&Option<InProgress>,b:&Option<InProgress>,first:Seq<u8>,second:Seq<u8>)
    requires super::checkpoint::same_progress(*a,*b), progress_encoded(a,first), progress_encoded(b,second),
    ensures first == second,
{
    if let (Some(x),Some(y)) = (a,b) {
        let vx = choose|view:Seq<u8>| view_codec::view_encoded(&x.target,view)
            && first == seq![1u8]+view+encoded_hashes(x.delivery_heads@)+vstd::bytes::spec_u64_to_le_bytes(x.applied);
        let vy = choose|view:Seq<u8>| view_codec::view_encoded(&y.target,view)
            && second == seq![1u8]+view+encoded_hashes(y.delivery_heads@)+vstd::bytes::spec_u64_to_le_bytes(y.applied);
        view_codec::view_encoding_unique(&x.target,&y.target,vx,vy);
    }
}
pub(crate) fn encode_progress(out:&mut Vec<u8>,p:&Option<InProgress>)
    ensures exists|bytes:Seq<u8>| progress_encoded(p,bytes) && final(out)@ == old(out)@+bytes,
{
    let ghost prefix = out@;
    match p {
        None => { out.push(0); assert(progress_encoded(p,seq![0u8])); },
        Some(progress) => {
            out.push(1);
            let ghost before = out@;
            progress.target.encode_into(out);
            let ghost view = choose|view:Seq<u8>| view_codec::view_encoded(&progress.target,view) && out@ == before+view;
            encode_hashes(out,&progress.delivery_heads);
            encode_u64(out,progress.applied);
            proof {
                let bytes = seq![1u8]+view+encoded_hashes(progress.delivery_heads@)+vstd::bytes::spec_u64_to_le_bytes(progress.applied);
                assert(progress_encoded(p,bytes));
                assert(out@ == prefix+bytes);
            }
        },
    }
}
pub(crate) fn decode_progress(r:&mut Reader<'_>) -> (out:Result<Option<InProgress>,Error>)
    ensures match out {
        Ok(p) => parse_progress(old(r).unread()) == Some((progress_data(&p),final(r).unread())),
        Err(e) => e is Malformed && parse_progress(old(r).unread()) is None,
    },
{
    proof { reveal(parse_progress); }
    if read_bool(r)? {
        let target = ViewPoint::decode_from(r)?;
        let ghost before = r.unread();
        let delivery_heads = r.hashes()?;
        proof { view_codec::hashes_match(delivery_heads@,before); }
        let applied = r.u64_le()?;
        Ok(Some(InProgress { target,delivery_heads,applied }))
    } else { Ok(None) }
}
} // verus!
