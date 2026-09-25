#import "@preview/fletcher:0.5.8" as fletcher: diagram, node, edge

= Ancestor Vectors

== Motivation

`stockfoil` needs to be able to support queries over the ancestors of various
parts of a commit graph. A general usecase here is the desire to diff between
different statues of the document as represented by a combination of two things

- The heads of the commit graph
- The write frontiers

The heads of the commit graph is straightforward, it's just the tips of the
commit graph. The write frontiers is a mapping from author ID (each author has
an Ed25519 signing key as it's author) to the commit hashes which are the
frontier of the down-set accepted for that author (or unbounded if there is no
restriction). The write frontier is different to the heads because when we 
encounter a commit which is not inside the write frontier we don't want to 
stop traversal of the graph, we just want to indicate to the caller that the
commit is not visible.

This concept of visibility makes the diff APIs that we would like to offer 
more complex than might be immediately obvious. When the point in a document is
identified by just a set of heads then diffing between them is well expressed
by a topological walk of the commit graph from before to after with a flag
indicating whether the commit was added or removed. With visibility in the mix
we need to expand the walk and add another flag for the commit visibility 
before and afterwards. Specifically, we start the walk at the intersection of
the down-set defined by the before/after write frontiers and then walk towards
the union of the before/after heads. We annotate each commit in the walk with
it's before/after presence in the graph, and it's before/after visibility
according to the write frontiers.

This traversal is good for some usecases. In other scenarios though
materialization is split into two steps. Firstly, ingesting newly delivered
(causally ready) commits, and secondly filtering commits based on whether they
are visible before/after some viewpoint. For example, Automerge stores all 
operations ever received in a log and then materializes a view by walking over
this log and filtering out things which are not visible.

This second requirement means that we would like a simple way to check whether
a commit is visible at some point in the commit graph.

== Chain Clocks

One way of implementing ancestry checks is to add parent/child links (we
already have parent links) and traverse the graph. This is simple and slow,
especially in large graphs. Instead I think we should use something I am
calling an "chain clock". 

The idea here is to do something a bit like a vector clock. We assign a unique
number to each "chain" in the commit graph and then represent any position as
an array of `(chain number, position)` pairs. In this manner we can determine
ancestry via a bunch of integer comparisons rather than by walking the graph.

In order to do this we need a mechanism to decompose the commit graph into 
chains. Once we have this we can store a pair `(chain number, position)`
(which I call the "chain time") on each commit. Determining ancestry then 
becomes a matter of determining the chain clock for some given set of heads
and then we can use this clock to quickly filter out commits based on that
clock. This finally gives us a mechanism for converting a view point into two
clocks which can then be used to determine delivery and visibility before/after
some viewpoint.

=== Chain Decomposition

The chain decomposition of a commit graph is done greedily. Every time we 
insert a new commit into the graph we do the following:

- For each predecessor (in lexicographic order)
  - Look up the chain ID of the predecessor
  - Look up the max position for that chain
  - If the max position is the chain position of the predecessor then set the
    chain time of the new commit to (pred chain ID, tip + 1)
- If no predecessor at the tip of it's chain was found
  - create a new chain ID and set the chain time of this commit to
    (new chain ID, 1)

=== Cached Clocks

Storing the chain time on each commit is useful, but we still need to be able
to derive the full clock at each commit. Naively this can be done by walking
the commit graph from the root merging clocks. However, we have a natural
coarsening of the commit graph. I think we can actually cache the chain clocks
at the boundary of each fragment. Thus, to derive a clock for some arbitrary
commit we find it's nearest containing fragment (we may need an index for this,
or we could use the clocks maybe) and then walk forward accumulating the clocks
for each commit until we reach the target.
