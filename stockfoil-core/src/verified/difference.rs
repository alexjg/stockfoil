//! Complete chain-by-chain difference enumeration using the verified tree iterator.
use super::clock::NodeIdx;
#[cfg(verus_keep_ghost)]
use super::clock::below;
use super::{clock, tree::Clock};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) open spec fn selected(node: NodeIdx, after: NodeIdx, before: Option<NodeIdx>) -> bool {
    node@ <= after@ && !below(before, node@ as usize)
}

pub(crate) open spec fn selection(nodes: Seq<NodeIdx>, after: NodeIdx, before: Option<NodeIdx>) -> Seq<NodeIdx>
    decreases nodes.len(),
{
    if nodes.len() == 0 { Seq::empty() } else {
        let prior = selection(nodes.drop_last(), after, before);
        if selected(nodes.last(), after, before) { prior.push(nodes.last()) } else { prior }
    }
}

proof fn interval_is_selection(nodes: Seq<NodeIdx>, start: int, end: int, after: NodeIdx, before: Option<NodeIdx>)
    requires 0 <= start <= end <= nodes.len(),
        forall|i: int| 0 <= i < nodes.len() ==> (start <= i < end <==> selected(nodes[i], after, before)),
    ensures nodes.subrange(start, end) == selection(nodes, after, before),
    decreases nodes.len(),
{
    if nodes.len() > 0 {
        let n = nodes.len() - 1;
        let lo = if start < n { start } else { n };
        let hi = if end < n { end } else { n };
        interval_is_selection(nodes.drop_last(), lo, hi, after, before);
        assert(nodes.subrange(start, end) =~= if selected(nodes.last(), after, before) {
            nodes.drop_last().subrange(lo, hi).push(nodes.last())
        } else { nodes.drop_last().subrange(lo, hi) });
    }
}

pub(crate) open spec fn tables_view(tables: Seq<Vec<NodeIdx>>) -> Seq<Seq<NodeIdx>> {
    tables.map(|i: int, nodes: Vec<NodeIdx>| nodes@)
}

pub(crate) open spec fn difference_prefix(tables: Seq<Seq<NodeIdx>>, entries: Seq<(u32, NodeIdx)>, before: &Clock, n: nat) -> Seq<NodeIdx>
    recommends n <= entries.len(), forall|i: int| 0 <= i < entries.len() ==> entries[i].0 < tables.len(),
    decreases n,
{
    if n == 0 { Seq::empty() } else {
        let (chain, tip) = entries[(n - 1) as int];
        difference_prefix(tables, entries, before, (n - 1) as nat)
            + selection(tables[chain as int], tip, before.lookup(chain))
    }
}

pub(crate) proof fn selection_membership(nodes: Seq<NodeIdx>, after: NodeIdx, before: Option<NodeIdx>, node: NodeIdx)
    ensures selection(nodes, after, before).contains(node) == (nodes.contains(node) && selected(node, after, before)),
    decreases nodes.len(),
{
    broadcast use vstd::seq_lib::lemma_seq_contains_after_push;
    if nodes.len() > 0 {
        selection_membership(nodes.drop_last(), after, before, node);
        assert(nodes =~= nodes.drop_last().push(nodes.last()));
    }
}

pub(crate) proof fn prefix_membership(tables: Seq<Seq<NodeIdx>>, entries: Seq<(u32, NodeIdx)>, before: &Clock, n: nat, node: NodeIdx)
    requires n <= entries.len(), forall|i: int| 0 <= i < entries.len() ==> entries[i].0 < tables.len(),
    ensures difference_prefix(tables, entries, before, n).contains(node) <==>
        exists|i: int| 0 <= i < n && tables[entries[i].0 as int].contains(node)
            && selected(node, entries[i].1, before.lookup(entries[i].0)),
    decreases n,
{
    if n > 0 {
        prefix_membership(tables, entries, before, (n - 1) as nat, node);
        let last = entries[(n - 1) as int];
        selection_membership(tables[last.0 as int], last.1, before.lookup(last.0), node);
        let prior = difference_prefix(tables, entries, before, (n - 1) as nat);
        let tail = selection(tables[last.0 as int], last.1, before.lookup(last.0));
        assert((prior + tail).contains(node) == (prior.contains(node) || tail.contains(node))) by {
            if prior.contains(node) {
                let i = choose|i: int| 0 <= i < prior.len() && prior[i] == node;
                assert((prior + tail)[i] == node);
            }
            if tail.contains(node) {
                let i = choose|i: int| 0 <= i < tail.len() && tail[i] == node;
                assert((prior + tail)[i + prior.len()] == node);
            }
            if (prior + tail).contains(node) {
                let i = choose|i: int| 0 <= i < (prior + tail).len() && (prior + tail)[i] == node;
                if i < prior.len() { assert(prior[i] == node); }
                else { assert(tail[i - prior.len()] == node); }
            }
        }
        if exists|i: int| 0 <= i < n && tables[entries[i].0 as int].contains(node)
            && selected(node, entries[i].1, before.lookup(entries[i].0)) {
            let i = choose|i: int| 0 <= i < n && tables[entries[i].0 as int].contains(node)
                && selected(node, entries[i].1, before.lookup(entries[i].0));
            if i < n - 1 { assert(prior.contains(node)); }
        }
        if prior.contains(node) {
            let i = choose|i: int| 0 <= i < n - 1 && tables[entries[i].0 as int].contains(node)
                && selected(node, entries[i].1, before.lookup(entries[i].0));
            assert(0 <= i < n);
        }
    }
}

pub(crate) open spec fn delta_member(tables: Seq<Seq<NodeIdx>>, after: &Clock, before: &Clock, node: NodeIdx) -> bool {
    exists|c: u32| c < tables.len() && tables[c as int].contains(node)
        && after.contents().contains_key(c) && selected(node, after.contents()[c], before.lookup(c))
}

pub(crate) proof fn difference_membership(tables: Seq<Seq<NodeIdx>>, after: &Clock, before: &Clock, node: NodeIdx)
    requires after.ordered(), forall|c: u32| after.contents().contains_key(c) ==> c < tables.len(),
    ensures difference_prefix(tables, after.sequence(), before, after.sequence().len()).contains(node)
        == delta_member(tables, after, before, node),
{
    after.sequence_correct();
    prefix_membership(tables, after.sequence(), before, after.sequence().len(), node);
    if delta_member(tables, after, before, node) {
        let c = choose|c: u32| c < tables.len() && tables[c as int].contains(node)
            && after.contents().contains_key(c) && selected(node, after.contents()[c], before.lookup(c));
        let i = choose|i: int| 0 <= i < after.sequence().len() && after.sequence()[i].0 == c;
        assert(after.sequence()[i].1 == after.contents()[c]);
    }
    if difference_prefix(tables, after.sequence(), before, after.sequence().len()).contains(node) {
        let i = choose|i: int| 0 <= i < after.sequence().len() && tables[after.sequence()[i].0 as int].contains(node)
            && selected(node, after.sequence()[i].1, before.lookup(after.sequence()[i].0));
        let c = after.sequence()[i].0;
        assert(after.contents().contains_key(c));
    }
}

pub(crate) fn difference(chain_nodes: &[Vec<NodeIdx>], after: &Clock, before: &Clock, out: &mut Vec<NodeIdx>)
    requires after.ordered(), before.ordered(),
        forall|c: u32| after.contents().contains_key(c) ==> c < chain_nodes.len(),
        forall|c: int| 0 <= c < chain_nodes.len() ==> clock::ordered(chain_nodes[c]@),
    ensures final(out)@ == old(out)@ + difference_prefix(tables_view(chain_nodes@), after.sequence(), before, after.sequence().len()),
        forall|node: NodeIdx| final(out)@.subrange(old(out)@.len() as int, final(out)@.len() as int).contains(node)
            == delta_member(tables_view(chain_nodes@), after, before, node),
{
    proof { after.sequence_correct(); }
    let ghost initial = out@;
    let ghost mut done: nat = 0;
    let mut iter = after.entries();
    loop
        invariant after.ordered(), before.ordered(), done <= after.sequence().len(),
            iter.remaining() == after.sequence().skip(done as int),
            out@ == initial + difference_prefix(tables_view(chain_nodes@), after.sequence(), before, done),
            forall|c: u32| after.contents().contains_key(c) ==> c < chain_nodes.len(),
            forall|c: int| 0 <= c < chain_nodes.len() ==> clock::ordered(chain_nodes[c]@),
        ensures out@ == initial + difference_prefix(tables_view(chain_nodes@), after.sequence(), before, after.sequence().len()),
        decreases iter.remaining().len(),
    {
        let coordinate = iter.next();
        match coordinate {
            None => break,
            Some((chain, tip)) => {
                proof {
                    after.sequence_correct();
                    assert(after.sequence()[done as int] == (chain, tip));
                }
                let nodes = &chain_nodes[chain as usize];
                let previous = before.position(chain);
                let (start, end) = clock::difference_range(nodes, tip, previous);
                proof { interval_is_selection(nodes@, start as int, end as int, tip, previous); }
                let ghost prior = out@;
                let mut i = start;
                while i < end
                    invariant start <= i <= end <= nodes.len(), out@ == prior + nodes@.subrange(start as int, i as int),
                    decreases end - i,
                {
                    out.push(nodes[i]);
                    i += 1;
                    assert(out@ =~= prior + nodes@.subrange(start as int, i as int));
                }
                proof {
                    done = done + 1;
                    assert(out@ =~= initial + difference_prefix(tables_view(chain_nodes@), after.sequence(), before, done));
                }
            },
        }
    }
    proof {
        assert(out@.subrange(initial.len() as int, out@.len() as int) =~=
            difference_prefix(tables_view(chain_nodes@), after.sequence(), before, after.sequence().len()));
        assert forall|node: NodeIdx| out@.subrange(initial.len() as int, out@.len() as int).contains(node)
            == delta_member(tables_view(chain_nodes@), after, before, node) by {
            difference_membership(tables_view(chain_nodes@), after, before, node);
        }
    }

}

pub(crate) fn symmetric_difference(chain_nodes: &[Vec<NodeIdx>], a: &Clock, b: &Clock, out: &mut Vec<NodeIdx>)
    requires a.ordered(), b.ordered(),
        forall|c: u32| a.contents().contains_key(c) || b.contents().contains_key(c) ==> c < chain_nodes.len(),
        forall|c: int| 0 <= c < chain_nodes.len() ==> clock::ordered(chain_nodes[c]@),
    ensures final(out)@ == old(out)@
        + difference_prefix(tables_view(chain_nodes@), a.sequence(), b, a.sequence().len())
        + difference_prefix(tables_view(chain_nodes@), b.sequence(), a, b.sequence().len()),
{
    difference(chain_nodes, a, b, out);
    difference(chain_nodes, b, a, out);
}

} // verus!
