//! Canonical payload encoding and authenticated byte transport for the graph.
use crate::*;

impl<C: PayloadCodec> CommitGraph<C> {
    /// Budgets for untrusted inbound items only; local commits and exports are
    /// not constrained. Increase these explicitly to retry a rejected item.
    pub fn set_decode_limits(&mut self, limits: DecodeLimits) {
        self.limits = limits;
    }

    pub fn decode_limits(&self) -> &DecodeLimits {
        &self.limits
    }

    pub fn describe_commit(
        &self,
        author: Author,
        payload: &C::Payload,
    ) -> Result<CommitMeta, Error<C::Error>> {
        crate::wire::check_format(self.codec.format_id())?;
        let bytes = self.codec.encode_commit(payload).map_err(Error::Codec)?;
        Ok(self.commit_by(author, &bytes))
    }

    pub fn commit(
        &mut self,
        author: Author,
        payload: C::Payload,
    ) -> Result<Draft, Error<C::Error>> {
        let meta = self.describe_commit(author, &payload)?;
        let hash = *meta.hash();
        let report = self.insert_local_commit(meta, payload);
        Ok(Draft {
            hash,
            signing_bytes: *hash.as_bytes(),
            report,
        })
    }

    /// Decode and authenticate an encoded item before admitting or queueing it.
    pub fn ingest(&mut self, bytes: &[u8]) -> Result<IngestReport, Error<C::Error>> {
        let (item, mut payloads) = crate::wire::decode(&self.codec, &self.limits, bytes)?;
        let verified = item.verify().map_err(Error::Verification)?;
        self.ingest_verified(verified, |meta| {
            payloads
                .remove(meta.hash())
                .expect("decoded member payload")
        })
        .map_err(Error::Ingest)
    }

    fn encode(&self, item: UnverifiedItem) -> Result<EncodedItem, Error<C::Error>> {
        crate::wire::encode(&self.codec, item, |hash| {
            self.publishable_commit(hash).map(|(_, p)| p)
        })
    }

    /// Encode the certified inventory added since this destination's cursor.
    /// An empty cursor exports the complete current inventory. Retain the old
    /// cursor until every added item has been stored successfully; retrying it
    /// is safe. Retired IDs name superseded inventory entries, not invalid data.
    /// Unsigned local drafts and queued received items are never exported.
    pub fn export_changes(
        &self,
        since: &ExportCursor,
    ) -> Result<(ExportChanges, ExportCursor), Error<C::Error>> {
        let (changes, next) = self.export_items_since(since);
        let added = changes
            .added
            .into_iter()
            .map(|item| self.encode(item))
            .collect::<Result<_, _>>()?;
        Ok((
            ExportChanges {
                added,
                retired: changes.retired,
            },
            next,
        ))
    }

    /// Encoded transfer, never unchecked sharing of another codec's payloads.
    /// Only admitted history is transferred, not queues, drafts, or permissions.
    /// Like a sequence of ingests, earlier valid items remain on a later error.
    /// Failures retain all earlier admissions/deferred rejections and the current
    /// pending count in `MergeError::report`. Export/format failures do not mutate
    /// the receiver and carry its initial pending count.
    pub fn merge(&mut self, other: &Self) -> Result<IngestReport, MergeError<C::Error>> {
        let mut report = IngestReport {
            pending: self.pending_items(),
            ..IngestReport::default()
        };
        if self.codec.format_id() != other.codec.format_id() {
            return Err(MergeError {
                report,
                error: Error::WrongCodec,
            });
        }
        let (changes, _) = match other.export_changes(&ExportCursor::default()) {
            Ok(changes) => changes,
            Err(error) => return Err(MergeError { report, error }),
        };
        for item in changes.added {
            let next = match self.ingest(&item.bytes) {
                Ok(next) => next,
                Err(error) => {
                    report.pending = self.pending_items();
                    return Err(MergeError { report, error });
                }
            };
            report.admitted.extend(next.admitted);
            report.rejected.extend(next.rejected);
        }
        report.pending = self.pending_items();
        Ok(report)
    }
}
