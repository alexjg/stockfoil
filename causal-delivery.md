# Causal Delivery

One annoying thing that any library building on a commit graph has to do is 
handle causal delivery. This is typically managed by buffering commits
somewhere and then delivering them when all their dependencies have arrived.

I want stockfoil to be able to handle this for users. 

This is complicated by the fact that I also want to use stockfoil to take
advantage of sedimentree. Specifically I want to allow loading an entire
fragment of a sedimentree without regenerating the commit graph in the
fragment. The way this would work in Automerge is that every operation in the
fragment would refer to the commit it is part of by index (or, more likely, by
actor ID and counter). The fragment would also contain a mapping from this
index or ID to the metadata about a given commit (parents, timestamps, etc.).
This means that the operations in the fragment (which are in a very specific
order) can just be immediately inserted into the document in a single walk of
the OpSet. The commit graph that Automerge maintains would then be by commit 
ID, not by hash.

How can we support this in stockfoil? Does `stockfoil` need modes of some kind?

I guess if we are using a commit hash as the commit ID then stockfoil can
generate and validate those commit IDs.

```rust
struct CommitHash(Vec<u8>);

fn load_audited(data: &[u8]) -> Vec<stockfoil::Commit<am::Change>> {
}

fn load_trusted(data: &[u8]) -> Vec<stockfoil::Fragment<am::Fragment>> {
}
```

Hmm, no, this doesn't help with causal delivery at all.

Let's say this is the trusted API:

```rust
let graph = stockfoil::CommitGraph::new();

let old_heads = graph.delivered_heads();
let changes: Vec<stockfoil::Commit<am::Change>> = stockfoil::load_changes(&data)?;
graph.ingest(changes);
let changes = graph.diff(&old_heads);
self.apply_changes_log_patches(&changes)
```

Okay, that works I think. It has unfortunately required loading all the changes
into memory, but automerge does that anyway.

What about untrusted:

```rust
type Fragment {
}

impl stockfoil::Fragment for Fragment { 
    type CommitId;
    
    fn commits(&self) -> impl Iterator<Item = stockfoil::Commit<Self::CommitId>>;
}

let frag_graph = stockfoil::FragmentGraph::new();

let old_heads = graph.delivered_heads();
let changes: Vec<stockfoil::Fragment<am::Fragment>> = stockfoil::load_fragments(&data)?;
frag_graph.ingest(&chages);
```

Okay no this is not going in the right direction. My thought was that the
sedimentree structure is the untrusted input because this is where we get the
benefit of not reifying the commits in order to hash them. It feels weird that
the only thing which really expresses that is that the commits are identified
by a different thing.

But actually maybe that's fine. I'm beginning to think that the commit
interface should actually be the other way around to what I'm thinking. Maybe
it's more like this:

```rust
trait Commit {
    type Id;
    type Payload;

    fn id(&self) -> &Self::Id;

    fn parents(&self) -> impl Iterator<Item = &Self::Id>;

    fn payload(&self) -> &Self::Payload;
}
```

And then the trusted graph always works with commits that have an ID type of 
`stockfoil::CommitHash`?

Hmm. No, I think it's worthwhile to have this be a data type.

```rust
stockfoil::Commit<CommitId, Payload>;
```

Ah, right, yes, the main point here is that we can have a commit ID. It can by
default be a commit hash. But it doesn't have to be. If it _is_ a commit hash,
then we have to reconstitute all the commits. How does stockfoil know that?

```rust

impl stockfoil::Commit for automerge::Change {
    type Id = (ActorId, Counter);
    fn id(&self) -> Self::Id;

    fn parents(&self) -> impl Iterator<Item = Self::Id> {
    }

    fn encode(&self, out: &mut Vec<u8>) {
    }

    fn decode(&self, data: &[u8]) -> Result<Self, Error> {
    }
}

impl stockfoil::Fragment for automerge::Bundle {
    type Id = (ActorId, Counter);
    fn id(&self) -> Self::Id {}

    fn boundary(&self) -> impl Iterator<Item = Self::Id> {}

    fn head(&self) -> Self::Id {}

    fn encode(&self, out: &mut Vec<u8>) {}

    fn decode(&self, data: &[u8]) -> Result<Self, Error> {}
}

impl Automerge {

    fn load_incremental(&mut self, data: &[u8], trusted: bool) {
        const chunks = stockfoil::parse::<automerge::Change>(&data);
        for chunk in chunks {
            match chunk {
                stockfoil::Fragment(frag) => {
                    if (trusted) {
                        self.graph.ingest(&frag)
                    } else {
                        let verified = frag.unbundle();
                        self.graph.ingest(&verified); 
                    }
                },
                stockfoil::Commit(commit) => {
                    self.graph.ingest(&commit);
                }
            }
        }
        let new_changes = ...
    }
}

```

Probably this should be called `CommitPayload` and `FragmentPayload` or
something. But I think it works.

## Mixing trusted and untrusted content

So, we have some kind of commit graph type object. It isn't a straightforward
graph though. We have a bunch of weirdness going on:

* Commits can be identified by both their ID and their hash
* Commits will always have an ID but only have a hash sometimes (e.g. if they
  were loaded from a loose commit or from a bundle that was verified)
* The graph is coarse grained into fragments
* In fragments which are loaded in trusted mode we don't even have the commits
  just the metadata and the compressed contents

I guess this is fine? It seems strange but I think we might need to work with
it for a bit.

Some things you might want to do:

* ensure that all commits are fully reified and verified before acceptance into
  the graph
* ensure that commits are _never_ fully reified and hashed and so on
* Make sure that hashes are cached every so often so that regenerating hashes
  can be done easily

Probably worth thinking about later

