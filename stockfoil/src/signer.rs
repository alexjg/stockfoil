use crate::{Author, CommitHash, Signature};

/// A local synchronous signer. For keys elsewhere, take drafts, sign outside
/// the library, then attach their signatures; no runtime or lock is imposed.
pub trait Signer {
    fn author(&self) -> Author;
    fn sign(&self, hash: &CommitHash) -> Signature;
}
impl Signer for ed25519_dalek::SigningKey {
    fn author(&self) -> Author {
        Author::from(self)
    }
    fn sign(&self, hash: &CommitHash) -> Signature {
        Signature::from(ed25519_dalek::Signer::sign(self, hash.as_bytes()))
    }
}
impl<S: Signer + ?Sized> Signer for &S {
    fn author(&self) -> Author {
        (**self).author()
    }
    fn sign(&self, hash: &CommitHash) -> Signature {
        (**self).sign(hash)
    }
}
