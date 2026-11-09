//! The actual Ledger storage. Planning is parametric in untouched receive/queue fields.
use super::{
    author::Author, checkpoint::Follow, history::History, view::ViewPoint,
    write_permissions::WritePermissions,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Everything about one document except its bytes.
///
/// The ledger owns history, write permissions, local drafts and a bounded queue
/// of verified items awaiting dependencies. Payloads are only caller-assigned
/// IDs. It never performs I/O, buffers a payload, or awaits a future.
///
/// Receive by servicing a [`begin_receive`](Self::begin_receive) admission's
/// read/decode requests, then [`admit`](Self::admit) it. Commit by feeding a
/// [`CommitBuilder`](crate::CommitBuilder) and your storage together, then [`commit`](Self::commit)
/// with the stored payload's ID. Export describes members and a wire header;
/// you encode from your storage with your own buffers and runtime.
///
/// For payloads in memory, use [`Document`](crate::Document) instead.
///
/// The default type parameters retain the real receive options and admission queue.
/// Planning is checked for arbitrary values of these untouched fields.
pub struct Ledger<
    #[cfg(not(verus_keep_ghost))] Options = crate::ReceiveOptions,
    #[cfg(verus_keep_ghost)] Options,
    #[cfg(not(verus_keep_ghost))] Queue = crate::Queue<crate::ItemId>,
    #[cfg(verus_keep_ghost)] Queue,
> {
    pub(crate) history: History,
    pub(crate) author: Author,
    pub(crate) options: Options,
    pub(crate) permissions: WritePermissions,
    pub(crate) policy_epoch: u64,
    pub(crate) queue: Queue,
}

impl<Options, Queue> Ledger<Options, Queue> {
    pub(crate) proof fn representation(&self)
        ensures self.history_view() == self.history, self.permissions_view() == self.permissions,
    {}
    pub closed spec fn history_view(&self) -> History { self.history }
    pub closed spec fn permissions_view(&self) -> WritePermissions { self.permissions }
    pub fn history(&self) -> (out: &History) ensures *out == self.history_view(), { &self.history }
    pub fn permissions(&self) -> (out: &WritePermissions) ensures *out == self.permissions_view(), { &self.permissions }
    pub fn view(&self, follow: &Follow) -> (out: ViewPoint)
        ensures self.history_view().follow_target(follow, &self.permissions_view(), &out),
    { self.history.follow_view(follow, &self.permissions) }
}

} // verus!
