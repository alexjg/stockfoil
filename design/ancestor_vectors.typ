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
more complex than might be immediately obvious. 


=== Diffs

A common operation for example is to ask what commits have become visible
between two sets of heads. For example, here's a commit graph:

#figure(
  align(center)[
    #diagram(
      spacing: 12mm,
      node-stroke: 1pt,
      node((0, 0), [A], radius: 4mm),
      node((1, 0), [B], radius: 4mm),
      edge((1, 0), (0, 0), "-|>"),
    )
  ],
  caption: [A commit  graph with two nodes, arrows point from parents to children]
)

The heads of this commit graph are (`B`). If we now recieve some new commits
`C` and `D` we might have a graph that looks like this:

#figure(
  align(center)[
    #diagram(
      spacing: 12mm,
      node-stroke: 1pt,
      node((0, 0), [A], radius: 4mm),
      node((1, 0), [B], radius: 4mm),
      node((2, 0), [C], radius: 4mm),
      node((3, 0), [D], radius: 4mm),
      edge((1, 0), (0, 0), "-|>"),
      edge((2, 0), (1, 0), "-|>"),
      edge((3, 0), (2, 0), "-|>"),
    )
  ],
)

The heads of the graph are now `D`. We would like to be able to say, well I 
last examined this graph at `B` and I want to see what is new.

=== Write Frontiers

In `stockfoil` we have the concept of a "write frontier". Each commit has an
author and each author has a region of the commit graph which we accept writes
to, their write frontier. This region is expressed as a set of heads.

From time to time the write frontiers of an author change. Thus, we will
sometimes want to determine what commits have become visible due to a change in
the write frontiers, even if no new  commits have been received.


