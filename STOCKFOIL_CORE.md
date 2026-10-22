# Separating `stockfoil-core` and `stockfoil`

## Goal

Split the repository into two crates so that all protocol, verification, and state-machine logic lives in `stockfoil-core` (for direct use from FFI bindings like Wasm and UniFFI), while `stockfoil` is a thin higher-level Rust wrapper providing traits like `Storage` and `Codec` and the synchronous `Graph` driver.

1. **Phase 1: Pure structural split** into `stockfoil-core/` and `stockfoil/` under a virtual workspace root.
2. **Phase 2: `DecodeItem` state machine in `stockfoil-core`**, moving stored-item header parsing, streaming member decoding, decoder protocol validation, and `PayloadCheck` verification out of `Graph::decode_item` (and `javascript/src/index.ts`) into `stockfoil-core`.

---

## Phase 1: Pure Structural Split

### 1. Workspace Layout

Convert the root `Cargo.toml` into a virtual workspace manifest and place `stockfoil-core` and `stockfoil` in their own directories:

```text
Cargo.toml                        # [workspace] members = ["stockfoil-core", "stockfoil", "javascript", "examples/mv-register", "subduction"]
stockfoil-core/
  Cargo.toml                      # name = "stockfoil-core"
  src/
    lib.rs                        # Core exports (everything except `graph`)
    admission.rs
    author.rs
    blob_hash.rs
    commit_hash.rs
    error.rs
    evidence.rs
    meta.rs
    reader.rs
    signature.rs
    verify.rs
    wire.rs
    wire/tests.rs
    write_permission.rs
    history/
      mod.rs
      admit.rs
      clock.rs
      export.rs
      local.rs
      publish.rs
      tests.rs
      view.rs
  tests/
    reference.rs                  # Pure `History` randomized reference-model tests
stockfoil/
  Cargo.toml                      # name = "stockfoil", depends on `stockfoil-core`
  src/
    lib.rs                        # `pub use stockfoil_core::*;` + `mod graph; pub use graph::*;`
    graph.rs                      # `Storage`, `Codec`, `Decoder`, `Encoder`, `Graph`, `MemoryStorage`, etc.
  benches/
    history.rs                    # Benchmarks both `History` and `Graph`
  examples/
    file_storage.rs
  tests/
    support/mod.rs
    fixtures/
      wire-v2-commit.hex
      wire-v2-fragment.hex
    admission.rs
    graph.rs
    large.rs
    placement.rs
    storage.rs
    sync.rs
    vectors.rs
```

### 2. Crate Responsibilities & Dependencies

- **`stockfoil-core`**:
  - Contains `History`, `Admission`, `CommitBuilder`, `Prepared`, `PayloadCheck`, `ExportPlan`, `ExportCursor`, `ExportItem`, `ViewPoint`, `ViewDiff`, `Changes`, `MaterializationCursor`, `wire`, domain types (`Author`, `BlobHash`, `CommitHash`, `Signature`, `WritePermission`, `BundleMeta`, `FormatId`, `Limits`, etc.), and `Error`.
  - No `std::io` in its public API.
  - Dependencies: `bijoux`, `blake3`, `ed25519-dalek`, `hexane`.
- **`stockfoil`**:
  - Re-exports `stockfoil_core::*`.
  - Defines `Storage`, `PayloadLocation<K>`, `MemoryStorage`, `Codec<S>`, `Decoder`, `Encoder<S>`, `DecodeProgress`, `EncodeProgress`, `Graph<C, S>`, and `Driven`.
  - Dependencies: `stockfoil-core = { path = "../stockfoil-core" }`.

### 3. Updating Workspace Members

- **`javascript/Cargo.toml` & `javascript/src/lib.rs`**:
  - Depend on `stockfoil-core = { path = "../stockfoil-core" }` in `[dependencies]`.
  - Keep `stockfoil = { path = "../stockfoil" }` in `[dev-dependencies]` for `javascript/tests/fixture.rs` (update its `#[path]` to `../../stockfoil/tests/support/mod.rs`).
  - Update imports in `javascript/src/lib.rs` from `stockfoil::` to `stockfoil_core::`.
- **`subduction/Cargo.toml`**:
  - Update `stockfoil` path to `{ path = "../stockfoil" }`.
- **`examples/mv-register/Cargo.toml`**:
  - Update `stockfoil` path to `{ path = "../../stockfoil" }`.
- **`fuzz/Cargo.toml`**:
  - Update `stockfoil` path to `{ path = "../stockfoil" }`.

### 4. Phase 1 Verification

- `cargo test --workspace`
- `cargo bench --bench history --no-run`
- `cargo run --example file_storage -- 1`
- `(cd javascript && npm test)`
- `cargo check --manifest-path fuzz/Cargo.toml --bins`

---

## Phase 2: `DecodeItem` State Machine in `stockfoil-core`

### 1. Problem

Reading member payloads back from a stored wire item currently lives in `Graph::decode_item` (`stockfoil/src/graph.rs`) and is re-implemented in TypeScript in `Graph.decodeItem` (`javascript/src/index.ts`). That logic includes:

1. Incrementally reading and parsing the wire v2 header (`wire::header_len` and `wire::decode_header`) against `FormatId` and `Limits`.
2. Stepping the application decoder over body chunks with bounded buffers.
3. Enforcing the decoder protocol contract (`consumed <= offered_input`, `produced <= capacity`, `Working` must make progress, `NeedInput` at EOF is a codec error, `Finished` before all wanted members is `Error::CorruptPayload`).
4. Tracking member indices (`EndMember`), stopping early once the last wanted member is decoded, and verifying each wanted member against its commit via `PayloadCheck`.

Moving this into a sans-IO state machine in `stockfoil-core` ensures FFI bindings and `Graph` share one implementation.

### 2. Core API (`stockfoil-core/src/decode.rs`)

Create a `DecodeItem` operation from `History`:

```rust
impl History {
    /// Start decoding the canonical payloads of `wanted` members (member index
    /// -> expected commit hash) from a stored wire item.
    ///
    /// Fails immediately with `Error::UnknownCommit` if any wanted commit has
    /// not been delivered to this history. Once created, the operation owns its
    /// `PayloadCheck`s and buffers and never borrows or needs `History` again.
    pub fn decode_item(
        &self,
        options: ReceiveOptions,
        wanted: impl IntoIterator<Item = (usize, CommitHash)>,
    ) -> Result<DecodeItem, Error>;
}
```

`DecodeItem` reuses `Ticket`, `Request<'a>` (`Read` and `Decode`), `Reply<'a>`, `ReplyError`, `DecodeStatus`, and `Counts`:

```rust
#[derive(Debug)]
pub enum DecodeNext<'a> {
    /// Execute this `Read` or `Decode` request and answer it with [`DecodeItem::reply`].
    Request(Ticket, Request<'a>),
    /// A request is outstanding; nothing else can proceed until its reply.
    AwaitReply,
    /// Canonical bytes produced for a wanted `member`.
    /// Call [`DecodeItem::next`] again after consuming `bytes` to continue.
    Chunk { member: usize, bytes: &'a [u8] },
    /// Every wanted member has been decoded and verified against its commit.
    Done,
    /// Terminal failure (`Malformed`, `Limit`, `WrongFormat`, `Codec`,
    /// `Storage`, `Protocol`, or `CorruptPayload`).
    Failed(&'a Error),
}

impl DecodeItem {
    pub fn next(&mut self) -> DecodeNext<'_>;
    pub fn reply(&mut self, ticket: Ticket, reply: Reply<'_>) -> Result<(), ReplyError>;
    pub fn request(&self) -> Option<(Ticket, Request<'_>)>;
    pub fn meta(&self) -> Option<&Arc<BundleMeta>>;
    pub fn counts(&self) -> Counts;
}
```

### 3. State Machine Behavior

1. **Initialization (`History::decode_item`)**:
   - Collects `wanted` into a `BTreeMap<usize, (CommitHash, PayloadCheck)>` by calling `self.check_payload(&commit)?` for each entry.
   - If `wanted` is empty, starts in `Stage::Done` (no requests issued).
   - Clamps `options.read_size` and `options.decode_capacity` to `1..=MAX_BUFFER` (same as `Admission`).
2. **Header Stage**:
   - Issues `Request::Read { offset, max: options.read_size }` until `wire::header_len(&prefix, &options.limits)?` is satisfied.
   - Parses `BundleMeta` via `wire::decode_header(&prefix[..header_len], &options.format, &options.limits)?`.
   - If `meta.len() <= last_wanted`, fails immediately with `Error::CorruptPayload(missing_commit)` before requesting any decoding.
   - Retains any trailing bytes after `header_len` as initial body input and transitions to the body stage.
3. **Body Stage**:
   - When `input` is exhausted and `!eof`, issues `Request::Read { offset, max: options.read_size }`.
   - Otherwise issues `Request::Decode { meta, input, eof, capacity: options.decode_capacity }`.
   - When `Reply::Decoded { consumed, output, status }` arrives:
     - Validates `consumed <= input.len()`, `output.len() <= capacity`, and `!(status == DecodeStatus::Working && consumed == 0 && output.is_empty())` (`Error::Protocol`).
     - If `status == DecodeStatus::NeedInput && eof`, fails with `Error::Codec`.
     - If `status == DecodeStatus::Finished`, fails with `Error::CorruptPayload(missing_commit)` for the next remaining wanted member.
     - If current `member` is in `wanted`:
       - Updates that member's `PayloadCheck` with `output`.
       - If `status == DecodeStatus::EndMember`, finishes the member's `PayloadCheck` **before** emitting `Chunk` (so a single-chunk corrupt member fails with `Error::CorruptPayload` without emitting any unverified bytes).
       - If `!output.is_empty()`, stages `Chunk { member, bytes }` to be returned by `next()`.
     - If `status == DecodeStatus::EndMember`, increments `member` (after any staged `Chunk` is yielded) and transitions to `Stage::Done` once `member > last_wanted`.

### 4. Updating Wrappers to Use `DecodeItem`

- **`stockfoil` (`Graph::decode_item`)**:
  - Replace the manual header/body/check loop in `Graph::decode_item` with a driver loop over `self.history.decode_item(self.options.clone(), wanted.iter().map(|(&i, &h)| (i, h)))?` that answers `Request::Read` and `Request::Decode`, forwards `DecodeNext::Chunk { member, bytes }` to `visit(member, bytes)`, and stops on `Done` or `Failed`.
- **`javascript` (`javascript/src/lib.rs` & `javascript/src/index.ts`)**:
  - Expose `History.decodeItem(wanted, options) -> DecodeItem` in `javascript/src/lib.rs`.
  - Add a `driveDecode` helper in `javascript/src/index.ts` (sharing request execution with `drive`) and use it in `Graph.payloads`, removing the manual TypeScript `decodeItem` loop.

### 5. Phase 2 Verification

- Existing `tests/storage.rs` tests (`items_are_admitted_and_read_through_opaque_keys`, `corrupt_stored_payloads_fail_reads`, `decoded_members_must_be_the_wanted_commits`).
- New unit/integration tests driving `DecodeItem` directly with a fake host (stale/wrong-kind replies, chunked reads/decodes, early stop after last wanted member, corrupt member payload rejection).
- `(cd javascript && npm test)`
