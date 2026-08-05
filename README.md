# Stockfoil

A library for building CRDTs on top of authenticated commit DAGs.

## A history lesson

This library grew out of the work I have been doing on
[Automerge](https://github.com/automerge/automerge). Automerge is implemented
by storing and transmitting the operations of the underlying CRDT in a commit
DAG. We've been working on authentication and authorization for Automerge
documents. The basic idea is that we sign every commit in the document and
then encrypt the commits and synchronize the encrypted commit graph.

This is conceptually very simple. What complicates it is that Automerge 
documents rely heavily on compressing the commit graph. We produce commits at
a very granular level (e.g. per-keystroke). To avoid generating and storing
a hash for every commit we encode bundles of commits together using a
run-length encoding and re-derive the hashes when we need them. Naively syncing
the entire commit graph will cause a 32x blow up in bandwidth usage (in typical
scenarios) due to having to transmit the commit hash.

To solve this we introduced
[sedimentree](https://github.com/inkandswitch/subduction/blob/main/design/sedimentree.md)
which works a little like a distributed LSM tree where peers which have the
cleartext can produce larger compressed runs for others to use. This design
allows us to incrementally sync encrypted CRDTs whilst retaining the low
bandwidth usage the compression affords us. 

Sedimentree solves the problem of metadata overhead due to commit hashes.
Unfortunately the requirements of the particular authentication mechanism we
have chosen introduce a new overhead. Recall that we are authenticating by 
signing every commit. The problem with this is that we now have to transmit
the signature for every commit. This is similar to the problem with hashes in
that it massively increases bandwidth usage for small, granular commits.

Happily, we can take advantage of typical editing patterns here. Typically
small granular commits (like keystrokes) are issued in long chains with
occasional concurrency. In a long chain of commits the hash of the final commit
in the chain covers everything previously in the chain. This means that we
only need to retain the signatures on commits where the author changes. 

Thus, in Automerge  we encode an "author" field on every commit. This is an
arbitrary byte array set by the user. We also add a signature to each commit,
but it is outside the bytes covered by the hash of the commit. This means that
when compressing ranges of commits together we can discard signatures on
commits where the parent commits have the same author and in this way we can
verify the author of every commit.

## Extracting the idea

All of this work is really quite independent of the Automerge CRDT and should
be useful anytime you have the following things:

* A data structure which can be expressed as a DAG of deltas encoded in some
  binary fashion
* Where the deltas themselves can be encoded more efficiently if encoded in
  batches

## Stockfoil

If you have a data structure which meets these requirements then you can use
stockfoil. You have to provide a trait which looks like this:

```rust
trait Commit {

}
```

And then you get out of it:

* An efficient binary format for encoding the commit graph as both individual
  commits and bundles of commits
* An implementation of the "fragments" structure that sedimentree expects
* A sans-IO state machine based API to the commit graph which works in a fixed
  amount of memory and spools everything to/from disk/network.
* An in-memory synchronous API to the commit graph
* An asynchronous API to the commit grap

