# Authors, Signatures, and Write Frontiers

`stockfoil` is intended to support authentication and authorization of commits.
The idea is that each commit has an "author", which is an Ed25519 signing key.
By signing each commit we are able to authenticate the author of the commit. We
combine this with the concept of a "write frontier" - which is the set of heads
a particular author is allowed  to write to. All these features together mean
that we can support local first access control - if an author is revoked from
a graph whilst partitioned then when we hear about the revocation later we can
re-evaluate the write frontier and reject any commits which are no longer valid.

Actually implementing this is complicated by the requirements of compression.
We want to be able to compress runs of commits together, omitting all but the
latest commit hashes. Shipping an authors key and signature for every commit
would be a huge overhead. Instead we do these three things:

* Allow commits to omit their author if at least one parent commit has the same
  author.
* Only retain the signature for the latest commit(s) by each author (i.e.
  each commit by some author which does not have a descendant by the same
  author)


