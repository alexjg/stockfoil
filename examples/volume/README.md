# Sparse-volume CRDT

A runnable, Unix example of **caller-owned I/O** over Stockfoil's `Ledger`
and `Materialization`. No storage traits, async runtime, transport dependency,
or whole-payload buffers are added to Stockfoil.

```sh
cargo run -p stockfoil-volume -- /path/to/demo-directory
cargo test -p stockfoil-volume
```

The demo creates two replicas, concurrent overlapping writes, a working sparse
image and an independent historical branch. It checks convergence, hides one
author to demonstrate fallback, and compares a lazy read with the working image.
The deterministic Ed25519 keys are **demo keys only**.

## Model and formats

One fixed-size volume, divided into **64 KiB blocks**. A commit assigns complete
contents to some blocks. For each block, take causally maximal participating
writers and break concurrent ties by the largest commit hash. A later writer
supersedes its ancestors regardless of hash ordering. Unwritten blocks read zero.

Canonical payload:

```text
u64 LE block count
count × u64 LE block index, strictly increasing and inside the volume
count × 65536 bytes of complete block contents
```

Bundle body: for each member, `u64 LE canonical length | canonical bytes`.
The format ID is `stockfoil/volume/v1`. There is no compression or delta base.
The fixed volume size is application configuration and must agree among peers.

Malformed lengths, overflow, duplicate/unordered indices and out-of-range writes
are permanent **per-materialization** rejections, not ledger rejection.
Validation completes before any row of that write is installed. Actual storage
I/O failures abort the transaction instead of rejecting an authenticated commit.
Invisible writes are indexed too; visible descendants may refer to them.

## Components and memory

- `Volume`: ledger, immediate synchronous signing, durable wire-item archive.
  Rebuilds history from archived items on open; permissions are supplied by the
  host, not synchronized or persisted as volume policy.
- `Spool`: append-only canonical bytes; payload IDs are byte offsets. Region
  metadata and item/member caches live in a SQLite catalog. Data is synced before
  catalog references commit. Recovery re-verifies wire bytes while reusing
  cached canonical regions, rather than appending payload copies on every open.
- `Buffers`: caller-owned read, decode and one-block buffers, reused throughout
  commit, admission, export, journal preparation and image replay.
- `BodyDecoder`: constant-sized framing state. Receive drives actual
  `Admission` read/decode requests with the caller's explicit buffer sizes.
- `BlockIndex`: SQLite rows for delivered writes, block ordinals, participation,
  winners and its checkpoint. Bounded page caches and file-backed temporary
  tables keep even the block-map metadata out of process-sized collections.
- `DiskImage`: its own independently advancing index/planner plus a flat sparse
  file. Each `step` processes a bounded number of source records and finishes all
  correction/payload rounds before persisting. Older winners use actual
  `Applied::wanted` / `Event::Payload` follow-ups.
- Lazy reads use `BlockIndex::read_at` and owned `Ancestry` selections under the
  ledger's current document-level permissions, without reifying another image.

Graph-sized metadata (including region/cache entries and causal-winner frontiers)
may remain in memory. Payloads, dirty-block sets and per-block indexes do not.
A source-record budget is **not a byte budget**: applying one huge commit still
takes a whole transaction. Cancel between steps, or drop/reopen after interrupted
I/O; this example does not checkpoint within a single commit.

## Image/index/checkpoint atomicity

Simply fsyncing an image and unrelated checkpoint files would be incorrect.
Each image owns a SQLite index with a generation number and a durable redo log:

1. Begin a SQL transaction. Apply planner batches and all follow-up rounds to
   its index. Stream changed blocks into `journal.tmp`; no image bytes change.
2. Seal the journal with a versioned header, generation, volume size, operations
   and BLAKE3 checksum. Fsync it, rename to `journal`, fsync the directory.
3. Commit **index, checkpoint and generation together** in SQLite
   (`synchronous=EXTRA`, rollback journal). Sync the database directory before
   touching the image, including the rollback-journal deletion.
4. Replay the durable redo log into `image.raw`, then fsync the image.
5. Remove the redo log and fsync the directory.

On open, SQLite first recovers its own transaction. If the redo log's generation
is one ahead of the database, SQL never committed: discard it; the old image is
untouched. If generations match, replay the entire log idempotently before
exposing the image. A crash halfway through overwriting a block is safe to redo.
Bad checksums, unknown versions/generations and missing/truncated committed
images fail closed.

Handles are exclusively file-locked. Failed image updates poison the handle and
retain its last successful in-memory checkpoint; drop and reopen before reading
or continuing. Never read `image.raw` externally during updates/recovery.
Independent images use independent directories.

Volume mutation similarly becomes visible to materializers only after archive
files, canonical regions and catalog references are durable. If that persistence
fails after ledger mutation, the volume handle is poisoned and materialization
refuses it. Unsigned drafts are not recoverable from items; this example signs
immediately rather than promising draft durability.

The protocol assumes trusted local storage, working Unix file locks, atomic
same-filesystem rename and truthful file/directory fsync. Deterministic fault
tests exercise interruption boundaries; they are not hardware power-cut tests.
Create storage beneath a durable existing parent directory. Network filesystems
with weaker locking or durability guarantees are outside this example's contract.

## Tests and limits

Tests cover overlap/convergence, revocation and bounded frontiers, inactive
metadata, independent pinned branches, reordered history rebuild and partial
checkpoint resume, malformed writes, real I/O failure, duplicate parking/
eviction, canonical-spool reuse, and interruptions before journal publication,
after publication, after SQL commit, inside a block write and after image sync.

The ignored Linux test streams **2 GiB** through commit, export/receive and image
reification, asserting RSS high-water growth below 32 MiB. It requires about
12 GiB of disk and should **not** run with a small tmpfs-backed `/tmp`.
Run it on a local filesystem with sufficient space:

```sh
mkdir -p /large/local/disk/volume-tests
TMPDIR=/large/local/disk/volume-tests cargo test -p stockfoil-volume --release \
  multi_gib_streaming_has_flat_rss -- --ignored --nocapture
```

This is an application example, not a production filesystem: no garbage
collection, archive compaction, delta compression, local draft journal, arbitrary
byte-range writes or within-commit resumability. Orphaned append tails, abandoned
temporary files and retired wire items can remain on disk. Recovery can make
multiple verification passes when archive order and capped parking require it;
a durable topology index is a future optimization.
