# Sedimentree Sync

Stockfoil is designed to sync CRDTs via
[subduction](https://github.com/inkandswitch/subduction). Subduction expects
the commit graph of the CRDT to be expressed as "fragments" and
"loose commits". The idea here is that we bundle a document into an
increasingly large fragments. Each fragment is defined by a head commit and
a set of ancestor commits of the head commit which we call the "boundary". The 
commits in the fragment are found by walking backwards from the head to the
boundaries.

For sedimentree we define the head and boundary commits based on hash hardness.
A depth 2 fragment is one where the head and boundary commits have at least two
leading zero bytes in their hash. This means that nodes with
overlapping commit graphs will agree on the fragments in the overlapping
regions. With some redundancy.

From stockfoils perspective this means that we need to be able to produce
the sedimentree decomposition of the commit graph to be handed over to 
subduction and we need to be able to accept that same encoding from subduction.

## Binary Format

Subduction expects a binary format for commits and for fragments. The idea here
is that the CRDT knows a lot about how it could be encoded efficiently and 
how runs of commits can be encoded efficiently. For stockfoil this means that
we need to provide an extension point for the CRDT implementation to define
how to encode and decode commits and commit bundles.

## Sync without subduction

Subduction implements a bunch of the logic for detecting the missing pieces 
each ond of the wire needs. However, we won't always have subduction available
and it's still useful to be able to do synchronization ourselves. For this
purpose we should have some way of ingesting a sedimentree summary (defined in
our own data types) from another stockfoil document and determining a list of
wants, needs, and fixups.

A "fixup" in this context is a statement of which loose commits or fragments
in the combined sedimentrees are redundant.
