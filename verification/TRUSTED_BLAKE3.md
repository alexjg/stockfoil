# Approved BLAKE3 boundary

The user explicitly chose to **trust BLAKE3's implementation**. The active proof
therefore treats the five declarations in
[`blake3_boundary.rs`](../stockfoil/src/verified/blake3_boundary.rs) as assumptions,
not proved functions. This boundary is separate from the 38 Rust/standard-library
contracts in [TRUSTED_STDLIB.md](TRUSTED_STDLIB.md).

| Contract | Actual Rust operation | Assumed normal-return behavior |
| --- | --- | --- |
| BLAKE3-TYPE-1 | Private `blake3::Hasher` field | Opaque streaming state with a logical sequence of input bytes. |
| BLAKE3-NEW-1 | `Hasher::new()` | Starts an unkeyed stream with no input bytes. |
| BLAKE3-CLONE-1 | `Hasher::clone()` | Independent state with the same input sequence. |
| BLAKE3-UPDATE-1 | `Hasher::update(bytes)` | Appends exactly the supplied bytes. |
| BLAKE3-FINISH-1 | `finalize().as_bytes()` | Returns the deterministic 32-byte BLAKE3 digest of that sequence. |

The digest is an uninterpreted mathematical function. Nothing assumes collision
resistance, injectivity, hash freshness, authenticity, signature validity or
application correctness. Panics, allocation failure and backend termination are
outside these normal-return contracts. Stockfoil's contracts continue to allow
duplicate hashes and errors after an edit.

The harness compiles and links the actual `blake3` 1.8.7 dependency; it does not
substitute a fake implementation. [trusted_blake3.json](trusted_blake3.json) pins
the adapter, production dependency/lock files, the isolated verification dependency
manifest/lock file, and the artifact-selection script. Cargo builds it with the
same Rust 1.99.1 toolchain used by Verus. The registry checksum in both lockfiles
identifies the approved crate. The dependency's own backend and build chain are
inside this trust boundary.

Stockfoil's bijou64 parent-count encoder, ordered parent streaming, author and
payload-digest placement, public builder operations, document publication and local
change flow are **checked code**, outside this boundary. No contract for `bijoux`
was added. Its existing encoder is used as a runtime regression oracle across all
tier transitions and many u64 values. Encoding proofs check the documented tier
offsets, tag and big-endian payload; they do not verify the dependency's decoder.

The native `Into<Arc<[u8]>>` used by public Document commit is modeled through that
actual conversion's contract. Arbitrary user conversions need not preserve some
independently imagined input bytes. Local change uses an explicit, separately
audited standard-library Vec-to-Arc conversion that preserves every byte.
