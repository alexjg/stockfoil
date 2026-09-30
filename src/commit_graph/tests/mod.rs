//! Algorithm regressions using deliberately independent metadata/payload fixtures.
//! These tests exercise private structured admission, not the byte trust boundary;
//! `tests/codec.rs` covers the public codec and wire contracts.
use std::marker::PhantomData;

use crate::{DecodeLimits, FragmentShape, IngestReport, ItemId, PayloadCodec};

/// A type-level payload marker, never an alternate graph or a usable wire codec.
/// Old algorithm fixtures intentionally need not match metadata payload hashes.
#[derive(Clone)]
pub(super) struct FixtureCodec<P>(PhantomData<P>);

impl<P> Default for FixtureCodec<P> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<P: Clone> PayloadCodec for FixtureCodec<P> {
    type Payload = P;
    type Error = std::convert::Infallible;

    fn format_id(&self) -> &str {
        "test/structured-fixture"
    }

    fn encode_commit(&self, _: &P) -> Result<Vec<u8>, Self::Error> {
        unreachable!("structured algorithm fixture is not a wire codec")
    }

    fn decode_commit(&self, _: &[u8], _: &DecodeLimits) -> Result<P, Self::Error> {
        unreachable!("structured algorithm fixture is not a wire codec")
    }

    fn encode_fragment(&self, _: &FragmentShape, _: &[&P]) -> Result<Vec<u8>, Self::Error> {
        unreachable!("structured algorithm fixture is not a wire codec")
    }

    fn decode_fragment(
        &self,
        _: &FragmentShape,
        _: &[u8],
        _: &DecodeLimits,
    ) -> Result<Vec<P>, Self::Error> {
        unreachable!("structured algorithm fixture is not a wire codec")
    }
}

// This alias names the actual production graph, with exactly the same state and
// algorithms. No wrapper, forwarding query methods, or alternate engine exists.
pub(super) type CommitGraph<P> = super::CommitGraph<FixtureCodec<P>>;

impl<P: Clone> CommitGraph<P> {
    /// Exchange fixture certificates and their synthetic payloads. Only compiled
    /// for unit tests; real graph merges always encode and authenticate bytes.
    fn merge_fixture(&mut self, other: &Self) -> IngestReport {
        let cover = other.minimal_cover();
        let items = cover
            .loose_commits
            .into_iter()
            .map(|commit| ItemId::Commit(commit.commit))
            .chain(
                cover
                    .fragments
                    .into_iter()
                    .map(|fragment| ItemId::Fragment(fragment.range)),
            );
        let mut report = self.ingest_report();
        for id in items {
            let item = other
                .export_item(&id)
                .expect("current cover entry")
                .verify()
                .expect("certified cover entry");
            let result = self
                .ingest_verified(item, |meta| {
                    other.store[&other.by_hash[meta.hash()]].clone()
                })
                .expect("certified cover entry");
            report.admitted.extend(result.admitted);
            report.rejected.extend(result.rejected);
            report.pending = result.pending;
        }
        report
    }
}

mod ancestor_vectors;
mod auth;
mod certificates;
mod commit_graph;
mod export_changes;
mod fragment_certificates;
mod ingest;
mod materialization;
mod orset;
mod sedimentree;
mod support;
mod view_point_diff;
mod verification;
