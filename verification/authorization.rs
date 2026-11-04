//! Authorization policy independent of clocks and chain assignments.
//! Integer nodes stand for resolved commit identities; hash resolution and
//! public error handling remain obligations of the production refinement.
use vstd::prelude::*;
use crate::graph::*;

verus! {

pub enum Permission {
    Denied,
    Unbounded,
    Bounded(ISet<int>),
}

pub open spec fn permission(perms: IMap<usize, Permission>, author: usize) -> Permission {
    if perms.dom().contains(author) { perms[author] } else { Permission::Denied }
}

pub open spec fn allowed(g: Graph, h: ISet<int>, perm: Permission, a: int) -> bool {
    match perm {
        Permission::Denied => false,
        Permission::Unbounded => true,
        Permission::Bounded(frontier) => exists|f: int|
            frontier.contains(f) && h.contains(f) && reaches(g, a, f),
    }
}

pub open spec fn visible(
    g: Graph, authors: Seq<usize>, heads: ISet<int>, perms: IMap<usize, Permission>, a: int,
) -> bool {
    0 <= a < authors.len() && history(g, heads).contains(a)
        && allowed(g, history(g, heads), permission(perms, authors[a]), a)
}

pub open spec fn valid_heads(g: Graph, heads: ISet<int>) -> bool {
    forall|h: int| heads.contains(h) ==> 0 <= h < g.len()
}

pub proof fn missing_or_denied_author_cannot_participate(
    g: Graph, authors: Seq<usize>, heads: ISet<int>, perms: IMap<usize, Permission>, a: int,
)
    requires 0 <= a < authors.len(),
        !perms.dom().contains(authors[a]) || perms[authors[a]] is Denied,
    ensures !visible(g, authors, heads, perms, a),
{}

pub proof fn unbounded_is_exactly_selected_history(
    g: Graph, authors: Seq<usize>, heads: ISet<int>, perms: IMap<usize, Permission>, a: int,
)
    requires 0 <= a < authors.len(), permission(perms, authors[a]) is Unbounded,
    ensures visible(g, authors, heads, perms, a) == history(g, heads).contains(a),
{}

pub proof fn available_bound_includes_itself(g: Graph, heads: ISet<int>, frontier: ISet<int>, f: int)
    requires history(g, heads).contains(f), frontier.contains(f),
    ensures allowed(g, history(g, heads), Permission::Bounded(frontier), f),
{
    let h = choose|h: int| heads.contains(h) && reaches(g, f, h);
    reaches_order(g, f, h);
    unfold_reaches(g, f, f);
}

pub proof fn other_author_permissions_are_irrelevant(
    g: Graph, authors: Seq<usize>, heads: ISet<int>, perms: IMap<usize, Permission>,
    changed_author: usize, replacement: Permission, a: int,
)
    requires 0 <= a < authors.len(), authors[a] != changed_author,
    ensures visible(g, authors, heads, perms.insert(changed_author, replacement), a)
        == visible(g, authors, heads, perms, a),
{}

// Includes unknown (out-of-range) and known-but-out-of-history frontier heads.
pub proof fn only_available_frontier_heads_matter(
    g: Graph, h: ISet<int>, first: ISet<int>, second: ISet<int>, a: int,
)
    requires first.intersect(h) =~= second.intersect(h),
    ensures allowed(g, h, Permission::Bounded(first), a)
        == allowed(g, h, Permission::Bounded(second), a),
{
    bounded_policy(g, h, first, a);
    bounded_policy(g, h, second, a);
}

pub proof fn growing_available_frontier_cannot_revoke(
    g: Graph, h: ISet<int>, first: ISet<int>, second: ISet<int>, a: int,
)
    requires first.intersect(h).subset_of(second.intersect(h)),
        allowed(g, h, Permission::Bounded(first), a),
    ensures allowed(g, h, Permission::Bounded(second), a),
{
    let f = choose|f: int| first.contains(f) && h.contains(f) && reaches(g, a, f);
    assert(first.intersect(h).contains(f));
    assert(second.contains(f));
}

pub proof fn selected_history_survives_append(
    g: Graph, parents: ISet<int>, heads: ISet<int>, a: int,
)
    requires valid_heads(g, heads),
    ensures history(g.push(parents), heads).contains(a) == history(g, heads).contains(a),
{
    if history(g, heads).contains(a) {
        let h = choose|h: int| heads.contains(h) && reaches(g, a, h);
        append_preserves_reachability(g, parents, a, h);
        assert(reaches(g.push(parents), a, h));
    }
    if history(g.push(parents), heads).contains(a) {
        let h = choose|h: int| heads.contains(h) && reaches(g.push(parents), a, h);
        append_preserves_reachability(g, parents, a, h);
        assert(reaches(g, a, h));
    }
}

pub proof fn captured_visibility_survives_append(
    g: Graph, parents: ISet<int>, authors: Seq<usize>, new_author: usize,
    heads: ISet<int>, perms: IMap<usize, Permission>, a: int,
)
    requires authors.len() == g.len(), valid_heads(g, heads),
    ensures visible(g.push(parents), authors.push(new_author), heads, perms, a)
        == visible(g, authors, heads, perms, a),
{
    let h = history(g, heads);
    let next = g.push(parents);
    assert forall|f: int| history(next, heads).contains(f) == h.contains(f) by {
        selected_history_survives_append(g, parents, heads, f);
    }
    assert(history(next, heads) =~= h);
    if h.contains(a) {
        let head = choose|head: int| heads.contains(head) && reaches(g, a, head);
        reaches_order(g, a, head);
        assert(authors.push(new_author)[a] == authors[a]);
        if let Permission::Bounded(frontier) = permission(perms, authors[a]) {
            assert forall|f: int| h.contains(f) implies reaches(next, a, f) == reaches(g, a, f) by {
                let head = choose|head: int| heads.contains(head) && reaches(g, f, head);
                reaches_order(g, f, head);
                append_preserves_reachability(g, parents, a, f);
            }
            if allowed(g, h, Permission::Bounded(frontier), a) {
                let f = choose|f: int| frontier.contains(f) && h.contains(f) && reaches(g, a, f);
                assert(reaches(next, a, f));
            }
            if allowed(next, h, Permission::Bounded(frontier), a) {
                let f = choose|f: int| frontier.contains(f) && h.contains(f) && reaches(next, a, f);
                assert(reaches(g, a, f));
            }
        }
    }
}

pub proof fn bounded_policy(g: Graph, h: ISet<int>, frontier: ISet<int>, a: int)
    ensures allowed(g, h, Permission::Bounded(frontier), a)
        == history(g, frontier.intersect(h)).contains(a),
{
    if allowed(g, h, Permission::Bounded(frontier), a) {
        let f = choose|f: int| frontier.contains(f) && h.contains(f) && reaches(g, a, f);
        assert(frontier.intersect(h).contains(f));
    }
    if history(g, frontier.intersect(h)).contains(a) {
        let f = choose|f: int| frontier.intersect(h).contains(f) && reaches(g, a, f);
        assert(frontier.contains(f) && h.contains(f));
    }
}

// No available frontier head means no grant, even if rejected heads have
// ancestors inside the delivered history.
pub proof fn unavailable_frontier_grants_nothing(
    g: Graph, h: ISet<int>, frontier: ISet<int>, a: int,
)
    requires forall|f: int| frontier.contains(f) ==> !h.contains(f),
    ensures !allowed(g, h, Permission::Bounded(frontier), a),
{}

} // verus!
