//! User-approved BLAKE3 API boundary. These contracts are trusted, not proved.
//! No collision resistance, injectivity, or authentication property is assumed.
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
verus! {

pub uninterp spec fn digest(bytes:Seq<u8>) -> [u8;32];

// TRUST BLAKE3-TYPE-1: opaque state of the pinned blake3::Hasher implementation.
#[verifier::external_body]
#[derive(Debug)]
pub(crate) struct Blake3 { inner:blake3::Hasher }
impl Blake3 {
    pub uninterp spec fn bytes(&self) -> Seq<u8>;
    // TRUST BLAKE3-NEW-1: unkeyed initialization has no input bytes.
    #[verifier::external_body]
    pub(crate) fn new() -> (out:Self) ensures out.bytes() == Seq::<u8>::empty(),
    { Self { inner:blake3::Hasher::new() } }
    // TRUST BLAKE3-UPDATE-1: normal return appends exactly the supplied bytes.
    #[verifier::external_body]
    pub(crate) fn update(&mut self,bytes:&[u8])
        ensures final(self).bytes() == old(self).bytes() + bytes@,
    { self.inner.update(bytes); }
    // TRUST BLAKE3-FINISH-1: deterministic 32-byte unkeyed BLAKE3 digest.
    #[verifier::external_body]
    pub(crate) fn finish(&self) -> (out:[u8;32]) ensures out == digest(self.bytes()),
    { *self.inner.finalize().as_bytes() }
}
impl Clone for Blake3 {
    // TRUST BLAKE3-CLONE-1: independent clone of the exact streaming state.
    #[verifier::external_body]
    fn clone(&self) -> (out:Self) ensures out.bytes() == self.bytes(),
    { Self { inner:self.inner.clone() } }
}
} // verus!

#[cfg(test)]
mod tests {
    #[test]
    fn streaming_adapter_and_independent_clones_match_the_pinned_backend() {
        for len in [0, 1, 63, 64, 65, 1023, 1024, 1025, 8192] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let mut hash = super::Blake3::new();
            for chunk in data.chunks(17) {
                hash.update(chunk);
            }
            let mut cloned = hash.clone();
            assert_eq!(hash.finish(), *blake3::hash(&data).as_bytes());
            cloned.update(b"suffix");
            let mut extended = data.clone();
            extended.extend_from_slice(b"suffix");
            assert_eq!(cloned.finish(), *blake3::hash(&extended).as_bytes());
            assert_eq!(hash.finish(), *blake3::hash(&data).as_bytes());
        }
    }
}
