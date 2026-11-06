use crate::{
    Author, Codec, CommitHash, Document, Draft, DriveError, Error, Export, ExportCursor, Follow,
    Materialization, Materializer, Progress, Received, Signed, Signer, WritePermissions,
};

/// An in-memory document and one working-copy state, kept current after
/// successful mutations. Application failures may poison its driver: restore
/// or rebuild before using that state again. Additional checkouts use
/// independent materialization planners.
///
/// Driving methods return [`DriveError`], preserving materializer errors.
/// They are not transactions: a commit, receipt or policy change can already
/// be recorded when updating application state fails. Restore or rebuild the
/// state rather than blindly repeating the mutation.
pub struct Replica<C: Codec, M: Materializer> {
    document: Document<C>,
    state: M,
    working: Materialization,
}
impl<C: Codec, M: Materializer> Replica<C, M> {
    /// `state` must be empty. No I/O, runtime, or synchronization is introduced.
    pub fn new(codec: C, author: Author, state: M) -> Self {
        let supports = state.supports_visibility_changes();
        Self {
            document: Document::new(codec, author),
            state,
            working: Materialization::new(Follow::Working, supports),
        }
    }
    pub fn from_parts(document: Document<C>, state: M, working: Materialization) -> Self {
        Self {
            document,
            state,
            working,
        }
    }
    pub fn into_parts(self) -> (Document<C>, M, Materialization) {
        (self.document, self.state, self.working)
    }
    pub fn state(&self) -> &M {
        &self.state
    }
    pub fn heads(&self) -> &[CommitHash] {
        self.working.heads()
    }
    pub fn document(&self) -> &Document<C> {
        &self.document
    }
    /// Mutations through this escape hatch require a subsequent `update`.
    pub fn document_mut(&mut self) -> &mut Document<C> {
        &mut self.document
    }
    pub fn materialization(&self) -> &Materialization {
        &self.working
    }
    /// Rebuild from empty state, also the recovery path for a poisoned working copy.
    pub fn rebuild(&mut self, state: M) -> Result<Progress, DriveError<M::Error>> {
        self.working = Materialization::new(Follow::Working, state.supports_visibility_changes());
        self.state = state;
        self.update()
    }
    pub fn update(&mut self) -> Result<Progress, DriveError<M::Error>> {
        self.working.update(&self.document, &mut self.state)
    }
    pub fn change_draft<E: std::fmt::Display>(
        &mut self,
        edit: impl FnOnce(&mut M) -> Result<Option<Vec<u8>>, E>,
    ) -> Result<Option<Draft>, DriveError<M::Error, E>> {
        if !self.working.is_current(self.document.ledger()) {
            self.update().map_err(DriveError::with_edit_error)?;
        }
        self.working
            .change(&mut self.document, &mut self.state, edit)
    }
    pub fn change<E: std::fmt::Display>(
        &mut self,
        signer: &impl Signer,
        edit: impl FnOnce(&mut M) -> Result<Option<Vec<u8>>, E>,
    ) -> Result<Option<CommitHash>, DriveError<M::Error, E>> {
        if signer.author() != self.document.author() {
            return Err(Error::Protocol("signer differs from local author").into());
        }
        let draft = self.change_draft(edit)?;
        self.sign_drafts(signer)
            .map_err(DriveError::with_edit_error)?;
        Ok(draft.map(|d| d.hash))
    }
    pub fn commit(&mut self, canonical: &[u8]) -> Result<Draft, DriveError<M::Error>> {
        let draft = self.document.commit(canonical, None)?;
        self.update()?;
        Ok(draft)
    }
    pub fn sign_drafts(&mut self, signer: &impl Signer) -> Result<Signed, DriveError<M::Error>> {
        let result = self.document.sign_drafts(signer)?;
        self.update()?;
        Ok(result)
    }
    pub fn receive(&mut self, item: &[u8]) -> Result<(Received, Progress), DriveError<M::Error>> {
        let received = self.document.receive(item)?;
        let progress = self.update()?;
        Ok((received, progress))
    }
    pub fn set_permissions(
        &mut self,
        permissions: WritePermissions,
    ) -> Result<Progress, DriveError<M::Error>> {
        self.document.set_permissions(permissions);
        self.update()
    }
    pub fn export(&self, cursor: &ExportCursor) -> Export<'_, C> {
        self.document.export(cursor)
    }
    pub fn checkout(
        &self,
        mut state: M,
        follow: Follow,
    ) -> Result<(M, Materialization), DriveError<M::Error>> {
        let mut materialization = Materialization::new(follow, state.supports_visibility_changes());
        materialization.update(&self.document, &mut state)?;
        Ok((state, materialization))
    }
}
