//! Actual public checkpoint serialization and transfer into the resumed driver.
use vstd::prelude::*;
use crate::{checkpoint::Checkpoint, materialization_driver::Materialization};

verus! {

pub fn public_checkpoint_roundtrip(checkpoint:&Checkpoint) {
    let bytes = checkpoint.to_bytes();
    let decoded = Checkpoint::from_bytes(&bytes).unwrap();
    proof { decoded.same_codec_data(checkpoint); }
    assert(decoded.same(checkpoint));
    assert(decoded.rejected_view() == checkpoint.rejected_view());
    assert(crate::checkpoint::same_progress(decoded.progress_view(),checkpoint.progress_view()));
}

pub fn deterministic_checkpoint_bytes(a:&Checkpoint,b:&Checkpoint)
    requires a.same(b),
{
    let first = a.to_bytes();
    let second = b.to_bytes();
    proof { a.encoding_deterministic(b,first@,second@); }
    assert(first@ == second@);
}

pub fn public_checkpoint_input_classification(bytes:&[u8]) {
    let result = Checkpoint::from_bytes(bytes);
    assert((result is Ok) == (Checkpoint::decode_model(bytes@) is Some));
    if let Ok(checkpoint) = result {
        assert(Checkpoint::decode_model(bytes@) == Some(checkpoint.codec_data()));
    }
}

pub fn serialized_metadata_reaches_resume(checkpoint:&Checkpoint,supports_visibility_changes:bool) {
    let bytes = checkpoint.to_bytes();
    let decoded = Checkpoint::from_bytes(&bytes).unwrap();
    proof { decoded.same_codec_data(checkpoint); }
    let driver = Materialization::resume(decoded,supports_visibility_changes);
    let published = driver.checkpoint();
    assert(published.same(checkpoint));
    assert(driver.source_consistent());
    assert(driver.checkpoint_consistent());
    assert(driver.counters_consistent());
    assert(driver.fresh());
}
} // verus!
