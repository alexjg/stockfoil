//! Actual hash/permission representations interpreted by the independent policy.
use vstd::prelude::*;
use crate::{authorization::{self, Permission}, author::Author, clock::{NodeIdx, below},
    commit_hash::CommitHash, graph, tree::Clock, write_permission::WritePermission, write_permissions::WritePermissions};

verus! {

pub(crate) open spec fn selected(labels: Seq<u32>, reached: &Clock) -> ISet<int> {
    ISet::new(|n: int| 0 <= n < labels.len() && below(reached.lookup(labels[n]), n as usize))
}

pub open spec fn resolved(hashes: Map<CommitHash, NodeIdx>, frontier: Seq<CommitHash>) -> ISet<int> {
    ISet::new(|n: int| exists|i: int| 0 <= i < frontier.len()
        && hashes.contains_key(frontier[i]) && hashes[frontier[i]]@ == n)
}

pub(crate) proof fn resolved_singleton(hashes: Map<CommitHash, NodeIdx>, hash: CommitHash)
    requires hashes.contains_key(hash),
    ensures resolved(hashes, seq![hash]) == ISet::empty().insert(hashes[hash]@ as int),
{
    assert forall|n: int| resolved(hashes, seq![hash]).contains(n) == (n == hashes[hash]@) by {
        if resolved(hashes, seq![hash]).contains(n) {
            let i = choose|i: int| 0 <= i < seq![hash].len() && hashes.contains_key(seq![hash][i]) && hashes[seq![hash][i]]@ == n;
            assert(i == 0);
        }
        if n == hashes[hash]@ { assert(seq![hash][0] == hash); }
    }
    assert(resolved(hashes, seq![hash]) =~= ISet::empty().insert(hashes[hash]@ as int));
}

pub open spec fn model(hashes: Map<CommitHash, NodeIdx>, permission: WritePermission) -> Permission {
    match permission {
        WritePermission::Denied => Permission::Denied,
        WritePermission::Unbounded => Permission::Unbounded,
        WritePermission::Bounded(frontier) => Permission::Bounded(resolved(hashes, frontier@)),
    }
}

pub open spec fn rule(hashes: Map<CommitHash, NodeIdx>, permissions: &WritePermissions, author: Author) -> Permission {
    model(hashes, permissions.rule(author))
}

pub(crate) proof fn same_rule(hashes: Map<CommitHash, NodeIdx>, a: &WritePermissions, b: &WritePermissions, author: Author)
    requires a.same(b),
    ensures rule(hashes, a, author) == rule(hashes, b, author),
{
    assert(a.rule(author).same(&b.rule(author)));
    if let WritePermission::Bounded(left) = a.rule(author) {
        let right = b.rule(author)->Bounded_0;
        assert(left@ == right@);
    }
}

pub(crate) open spec fn visible(g: graph::Graph, labels: Seq<u32>, owners: Seq<Author>,
    hashes: Map<CommitHash, NodeIdx>, reached: &Clock, permissions: &WritePermissions, n: int) -> bool
{
    visible_in(g, labels, owners, hashes, selected(labels, reached), permissions, n)
}

pub open spec fn visible_in(g: graph::Graph, labels: Seq<u32>, owners: Seq<Author>,
    hashes: Map<CommitHash, NodeIdx>, selection: ISet<int>, permissions: &WritePermissions, n: int) -> bool
{
    selection.contains(n) && authorization::allowed(g, selection,
        rule(hashes, permissions, owners[labels[n] as int]), n)
}

} // verus!
