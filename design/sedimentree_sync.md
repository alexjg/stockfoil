# Sedimentree Sync

Stockfoil is designed to sync CRDTs via
[subduction](https://github.com/inkandswitch/subduction). Subduction expects
the commit graph of the CRDT to be expressed as "fragments" and
"loose commits". The idea here is that we bundle a document into an
increasingly large fragments. Each fragment is defined by a head commit and
a set of ancestor commits of the head commit which we call the "boundary". The 
members of a fragment are the head's inclusive history minus the union of
all boundary histories:

```
members = H(head) - union(H(boundary))
```

Boundaries themselves are excluded. An edge bypassing a boundary does not
reintroduce that boundary's ancestors into the payload.

For sedimentree we define the head and boundary commits based on hash hardness.
A depth 2 fragment is one where the head and boundary commits have at least two
leading zero bytes in their hash. This means that nodes with
overlapping commit graphs will agree on the fragments in the overlapping
regions. With some redundancy.

From stockfoils perspective this means that we need to be able to produce
the sedimentree decomposition of the commit graph to be handed over to 
subduction and we need to be able to accept that same encoding from subduction.

## Dependencies and verification

Boundary discovery remains a parent walk stopping at ancestors at least as deep
as the head. Redundant ancestor boundaries are retained. Member enumeration is
separate: ancestry clocks subtract all boundary history, using sorted per-chain
node positions to enumerate the remaining intervals.

`UnverifiedItem::Fragment` carries `range`, `additional_dependencies`, member
metadata, and signature evidence. Additional dependencies are exactly:

```
parents(members) - members - boundary
```

They account for parents excluded through boundary history, including bypass
edges. Declared prerequisites are the distinct union of boundaries and extras.
Signatures cover every supplied member; signature targets must themselves be
members, not prerequisites.

Verification is stateless: it checks signatures, item-local reachability,
acyclicity, depth constraints, and exact declared external-parent closure. It
also retains the boundary hashes directly referenced by supplied members in the
verified item. It cannot prove that an omitted parent belongs to boundary
history. Before payload callbacks or mutation, admission therefore:

1. Requires every declared dependency to be admitted, not merely a local draft.
2. Checks that extras lie in boundary history and supplied members do not.
3. Walks only additional dependencies' admitted history to discover further
   depth-based boundary stops, combining these with the cached direct references
   and requiring an exact match with the declared boundary.

Admission does not rebuild a member map or rewalk supplied member edges:
stateless reachability and closure, together with these placement checks,
already establish exact normalized membership.

Missing dependencies produce `IngestError::MissingDependency`; incorrect range
placement produces `IngestError::InvalidFragment`. The caller buffers/retries;
the graph has no received-item causal queue. These checks are mandatory even
when a correctly constructed source tree should guarantee the prerequisites.

## Structural summaries and certified covers

Structural summaries describe indexed canonical ranges. Their checkpoints and
loose-commit coverage use normalized members, never excluded boundary history.
Minimization preserves equal-depth overlaps. A deeper range can replace a
shallower one only when it contains the entire payload and each child boundary
is either a member or an explicit boundary of the covering range.

A checkpoint inside a received payload can head a canonical fragment containing
ancestors excluded from that received payload. Indexing that structural range
does not certify it for export. `CommitGraph::minimal_cover` therefore selects
only retained fragment certificates plus signed standalone commits, and can
differ from the minimized structural summary. Export, merge, and certificate
pruning use this certified cover. Boundary history remains independently
obtainable, and relays admit items in dependency order.

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
