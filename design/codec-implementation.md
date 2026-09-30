# Codec implementation notes

This document describes the current Rust/Wasm implementation. It is **not** the
wire specification: interoperating implementations should follow
[`codec-wire-v1.md`](codec-wire-v1.md), regardless of their language, dependencies,
allocation strategy, or API shape. [`CODEC.md`](../CODEC.md) describes the payload
codec and graph API contracts.

## Shared implementation and dependencies

`src/wire.rs` implements the envelope for native Rust and Wasm. A single
`CommitGraph<C: PayloadCodec>` owns history, its codec, and inbound limits;
`src/commit_graph/codec.rs` coordinates encoding and authenticated ingestion.
Structured admission and export helpers are private.

Metadata columns use pinned `hexane =1.0.0-alpha.5` with explicitly selected
LEB128 encoders and validating loaders. Hexane's types do not appear in the
application codec API. Canonical commit preimages use the existing Bijoux **u64**
parent-count encoding; this is distinct from the wire envelope's LEB128. The wire
specification defines both encodings without requiring either dependency.

Signature verification currently uses `ed25519-dalek`'s `verify_strict`, without
its `legacy_compatibility` feature. The normative signature profile is in the
wire specification, not implied by that method name. Dependency or feature
changes must preserve the specified acceptance rules as well as emitted bytes.

## Bounded column validation

The inspected Hexane alpha.5 RLE loader performs some arithmetic before returning
a run to the caller:

- `rle/decoder.rs`, `try_next_segment`: computes `(-n) as usize` for a literal
  run. Signed minimum overflows on negation; oversized casts can truncate on
  32-bit targets.
- `rle/load.rs`, `CutState::track`: increments the slab's decoded length with
  unchecked `usize` addition.

Consequently, bounding a run only after `try_next_run()` returns is insufficient.
Stockfoil first scans encoded headers/values without expanding repeat runs. This
preflight rejects signed minimum, checks conversions and cumulative counts,
requires the exact expected cardinality, and enforces integer spelling/value
constraints. It then uses the fallible validating loader for canonical run
structure, and bounds expansion into the output vector.

This is a workaround for the inspected dependency's arithmetic, not a mandated
protocol algorithm. A safe validating decoder could combine those checks into
one pass. Fixing the dependency would not eliminate Stockfoil's own need for
column cardinality, reference, and application resource checks.

## Local resource policy

Current `DecodeLimits` defaults are:

| Field | Default |
| --- | ---: |
| `max_item_bytes` | 8 MiB per envelope |
| `max_members` | 65,536 |
| `max_parents` | 1,000,000 flattened references |
| `max_signatures` | 65,536 |
| `max_payload_bytes` | 8 MiB total reconstructed canonical payload bytes |

The parent budget also separately bounds boundary and external-dictionary counts.
Author count is bounded by member count plus the signature budget. Fixed-width
table/signature counts are checked against available input before allocation.

These are inbound per-item policies, including receipt during native merge—not
wire-format maxima, local commit/export caps, or aggregate graph/queue limits.
Application decoders receive cooperative limits; the library cannot sandbox
arbitrary callback CPU, allocations, or external side effects. The pending queue
still has no aggregate quota.

A growing certified cover may exceed a receiver's budget after older certificates
have been pruned. Export remains available. A receiver can explicitly increase
its limits with `set_decode_limits` (JS: `setDecodeLimits`) and retry the same
bytes. This does not add authenticated chunking or a smaller fallback certificate.
See the [JS guide](../javascript/README.md#resource-budgets) for its partial-option
replacement semantics.

Export checks each canonical payload hash against retained metadata. Fragment
canonical bytes are hashed and dropped one member at a time, rather than retained
as a second complete batch. Export does not parse its own envelope or decode its
own fragment body to prove application codec losslessness.

Native merge processes encoded items sequentially. If a later item fails,
`MergeError::report` preserves earlier admissions and deferred rejections;
`MergeError::error` retains the cause. This API-level progress reporting is separate
from the wire protocol's item validity rules.

## Compression measurements

`src/wire/tests.rs` measures structure including framing, the claimed head,
dictionaries, and empty payload/signature framing. These metadata-size fixtures
are not complete authenticated messages:

| Structural workload | Measured bytes |
| --- | ---: |
| 10,000-member same-author chain | 106 |
| 1,000 members, four interleaved authors, two ordered internal parents per non-root member | 3,942 |

For the chain's internal-parent column, backward-distance RLE takes 4 bytes,
versus 6 bytes for delta-encoded absolute indices. For a binary-tree parent
workload with 9,999 internal edges, backward distances take 14,873 bytes, versus
10,002 for absolute deltas. Backward distances favor chains/local-neighbour edges,
not every topology; v1 fixes that representation independently of these results.

The complete signed 101-member [fragment vector](../tests/fixtures/codec-fragment-v1.hex)
is 256 bytes, carrying 4,617 canonical payload bytes with application-specific
run compression. Its schema and expected contents are specified under
[interoperability vectors](codec-wire-v1.md#9-interoperability-vectors).

Regression ceilings are below 115 bytes for the chain metadata fixture, 5,000 for
the mixed-author branching fixture, and 500 for the complete signed fragment.
These are regression expectations for particular inputs, not conformance limits.
Native, Node, and browser tests use the signed wire vectors; malformed-column,
identity, authentication, dependency-order, budget, and retry cases are tested
separately. Algorithm regressions use internal graph fixtures without exposing
unchecked graph mutation publicly.
