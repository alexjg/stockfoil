I'm worrying about big commits. Let's think about a large file storage scenario.

Let's say we want a sedimentree which represents an OS distribution. Several
gigabytes of data, with some very large files. One way you might model this is
with a single commit for the original image, then various deltas to this.

Let's say we go with the most naive possible encoding of things. The original
commit just contains a 3Gb image. Then each commit after that contains the 
updated files.

What are the things we might want to do?

## Generating new changes

In this case we're probably going to actually reify the image in the filesystem
and make changes to it. Maybe we record those mutations as we make them, maybe
instead we diff the filesystem after we've changed it. Either way, we have a
new state.

The new state could be too large to fit in memory. What we're probably going to
do is read the diff off of disk and spool it into a separate commit object on
disk. Then we're going to actually form the commit metadata, hash the whole
thing, sign it, and distribute it.

## Switching to different commits

What we basically want to do here is checkout a new state of the filesystem.
One way to form this state is to start at the root commit and then play forward
through the commits until we reach the desired state. What would that look like.

Well, provided we've constructed each commit so that it can be spooled into the
disk without being completely in memory what we will do is spool each commit
into memory, apply it to the disk, then once we've finished move on to the next
one.

If we're smart we can encode commits so that the deltas are described in the 
header of the commit so that we can read just the header of each commit as we
scan through and skip things which don't touch files we care about.

This last part is interesting. It means that we need the ability to read part
of a commit.

Git works quite differently. It has an object store which the commit log just
references. Should we do this? I don't think so. I think the right place to
have this kind of thing is at the collection level.

I do think we should probably store and transfer commit payloads in a separate
network message than the commit metadata.

## Rust API

Okay, let's think a bit about what this would look like in rust.

Let's say we're working on a CRDT which represents a project with some small
amount of metadata that lives in memory, but also some giant binary things.
Maybe something like a movie editor or something.

The rust API for this thing might look like this:

```rust

struct MovieDoc {}

impl MovieDoc {
    fn begin_apply(&mut self, meta: &stockfoil::CommitMetadata) -> Applying {}
    fn modify(&mut self) -> Modifying {}
}

struct Applying {}

impl Applying {
    fn handle_chunk(&mut self, chunk: &[u8]) -> Result<Next, ApplyError> {}
    fn finish(self) -> Result<CommitId, ApplyError> {}
}

enum Next {
    Continue,
    Done,
}

struct Modifying {}

impl Modifying {
    fn set_file_permissions(&mut self, filename: &str, perms: Permissions) {}
    fn begin_add_file(&mut self, filename: &str) -> AddingFile {}
}

impl AddingFile {
    fn append(&mut self, chunk: &[u8]) -> Result<(), Error> {}
}
```

Okay this is kind of the right direction. What does `AddingFile::append` 
actually do though? Presumably the caller is driving it by streaming stuff of
disk and then writing it to ... somewhere? Maybe we do something like this:

```rust
// in the stockfoil crate
trait Store {
    fn begin_blob(&self) -> BlobWriter;
    fn write_commit(&self, meta: CommitMeta, contents: BlobRef)

    fn get_blob(&self, blob_ref: &BlobRef) -> BlobReader;
}

trait BlobWriter {
    fn append(&self, data: &[u8]) -> Result<(), ..>;
    fn finish(self) -> BlobRef;
}

trait BlobReader {
    fn read(&self) -> Result<(), ..>;
}
```

This means that then `MovieDoc` can have a store in it and feed that all
the way through to the appendfile which can then 

```rust
struct MovieDoc<S: stockfoil::Store> {
    store: S,

}

impl Applying {

    fn handle_chunk(&mut self, chunk: &[u8]) -> Result<Next, ApplyError> {
        
    }
}
```

### Sans-io?

The design above is very not sans-IO. The `Store` trait is much more oriented 
towards a runtime. To design a sans-IO version we need to think about what
the states of the stockfoil components are.

Does stockfoil actually need to care about storage at all? There's an argument
that all stockfoil needs is to know that the caller can transactionally create
the commit metadata and the blob associated with it. The only thing stockfoil
needs to write is the commit metadata.

```rust
struct Stockfoil {}

impl Stockfoil {
    // Here we require that both the commit metadata and the data must have
    // been saved to storage.
    // 
    // What we have to do here is just add the commit to the internal graph.
    fn commit(&mut self, meta: CommitMetadata, data: BlobRef) {

}
```

What about applying incoming commits/fragments? What needs to be done in that
case? Let's say we receive some commit over the wire. Let's say we refer to
blobs via content addressing. Then all we need the caller to do is give us the
metadata about any new fragments which have been applied. 

```rust
impl Stockfoil {
    fn apply_fragment(&mut self, fragment: FragmentMeta) {}
}
```

This is sort of not different to the commit really. Okay, let's think this
through. What are the annoying things you don't want to care about when
designing a CRDT?

* How the commit metadata is encoded
* Assigning an author to a commit
* Handling signatures when compacting commits into fragments
* Encoding ranges of the commit graph as fragments
* Figuring out what commits need to be applied


```rust
impl MovieDoc {
    async fn apply_fragment(&mut self, data: BlobRef, meta: FragmentMeta) {

    }

    fn write_fragments_since(&mut self, heads: &[CommitHash], out: impl Writer) {

    }
}
```

### Fragment Compression

Okay, one thing to think about which is an absolute pain to implement from
scratch is fragment compaction. There are kind of two parts to this problem.
Firstly you need to identify which commits should be compacted, then secondly
actually compact the given commits.

Let's think about two different scenarios here. Firstly, how does Automerge do
it and secondly, how might something like `MovieDoc` do it?

#### Automerge

In automerge we have the entire commit graph in memeory (just the metadata,
not the contents). We use this representation to derive the start and end 
counters for the Op ID range we care about and then iterate over that range,
appending operations which appear in the commits to the operation metadata
chunk.

This all works because we have the commit graph and the full set of operations
in memory. Let's imagine what it would look like if neither of those things 
were true:

Firstly if we can't keep the whole commit graph in memory how do we determine
the start and end counters for the Op ID range? We need to turn each of the
commit hashes we have into some commit metadata and then get the counters from
that metadata. We can do this by having a local K/V cache of commits.

Secondly we would then stream over the operations from a local on disk cache
as well. 

This is actually not too bad.

Okay, next question. How do we generate new commits? In Automerge the way this
works is that as a transaction is progressing we add ops to an in memory queue
and then encode them all at once into a commit. We can't actually write the 
commit out to disk as we're making it, but we could write the ops to disk
as we go and then when we actually make the commit we could write the commit
header, then stream the ops into it, then seal the whole thing.

#### MovieDoc

What's the input to fragment compression? We have some set of commit boundaries
(head and tails) and we need to run the whole lot together into one chunk. We
can't have either the commit graph or the data in memory. What we do need is
some way of traversing the commits in topological order (like automerge). We
could do this naively by just loading each commit one at a time, this is going
to perform poorly on any real storage backend so we might want to externalize
that requirement somehow.

TODO: think more about externalizing topological traversal. Feels like an
important seam. In the automerge section above we assumed a local K/V cache
would solve the issue but that seems not at all obvious

Let's continue without it for now though. What we're going to do is travers
from the tails to the head in topological order. For each commit we're going
to stream in the commit contents and just concatenate it and stream the output
back out to some blob on the filesystem. Eventually we are going to write the
commit header.

## Disk Layout

I think part of what is confusing me here is that I am confusing implementation
with the actual shape of the problem. If we're trying to stay within a fixed
memory window, we need to think about what the layout of data on disk/storage
is.














