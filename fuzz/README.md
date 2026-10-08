# Stockfoil fuzzing

A standalone cargo-fuzz crate (its own `[workspace]`, so the main workspace
ignores it). It depends on `stockfoil` by path and builds against the
working copy.

```
fuzz/
  src/codec.rs        keys, identity rule, ChunkCodec (copy of tests/support)
  src/host.rs         fake foreign host: blob store, decoder, payload table, driver
  src/model.rs        naive whole-history reference model and checks
  src/templates.rs    valid items exported from a real graph (built once)
  fuzz_targets/       the four targets below
  examples/gen_seeds.rs   writes seed corpora
  regressions/<target>/   minimized crashing inputs
```

## Targets

| target | what it feeds | main invariants |
| --- | --- | --- |
| `wire_header` | arbitrary bytes to `wire::header_len` / `decode_header` with small `Limits`; odd mode byte builds a valid `BundleMeta`; bytes also go to the two cursor parsers | no panic; `header_len` verdicts are stable as bytes are appended; decoded metadata respects limits; `encode_header` re-decodes to an equal `BundleMeta` and is stable; cursor `to_bytes`/`from_bytes` round trip |
| `admission_bytes` | 9 config bytes (read size, decode capacity, quantum, host read/decode limits, small limits, pre-admitted template stage, flags), then a raw stored item, or (mode % 4 == 3) a crafted bundle signed with real keys | no panic; terminates; failed/waiting admissions leave `epoch()`/`len()` untouched; on success every member is admitted, heads/drafts/ancestry match the model, fragments are canonically placed, and the history re-exports into a fresh history that admits the same set with the same payload bytes; crafted bundles are checked against an independent validity predicate both ways |
| `admission_replies` | a valid template item; the fuzz input picks each host reply (correct, stale ticket, wrong kind, short read/decode, failure, garbage read/decode/bind, early EOF, empty read, wrong status, duplicate, deferral, wrong-history `advance`, wrong offset) | no panic; exactly the item's commits are admitted or the op fails (never waits); `Stale`/`WrongKind` change neither `counts()` nor the outstanding request; a failed op never mutates the history |
| `history_ops` | up to 64 operations on two `Graph<ChunkCodec, MemoryStorage>` replicas: commits (repeated parents, duplicates, depth-1/2 grinding), signing by right/wrong key or corrupted, partial reordered exports with acknowledgements, retries, change traversals and diffs with random heads and permission maps, full sync | model checks after every op (parent-closed admission, `heads`, `drafts`, `is_ancestor`, `changes` records, `diff`, `traverse_since`, cursor round trips); a draft signed by its author with admitted parents is admitted; full sync converges; payload bytes are exact |

## Running

```sh
cargo install cargo-fuzz          # once
cd fuzz
cargo +nightly run --example gen_seeds    # seed corpora under corpus/

# -a keeps debug assertions and overflow checks on (they are bugs too).
cargo +nightly fuzz run -a wire_header corpus/wire_header -- -max_total_time=600
cargo +nightly fuzz run -a admission_bytes corpus/admission_bytes -- -max_total_time=600 -max_len=4096
# These two only ever see valid items; ASan buys little, so run them faster:
cargo +nightly fuzz run -a -s none admission_replies corpus/admission_replies -- -max_total_time=600
cargo +nightly fuzz run -a -s none history_ops corpus/history_ops -- -max_total_time=600 -max_len=4096
```

Parallel, continuing past crashes (artifacts land in `artifacts/<target>/`):

```sh
cargo +nightly fuzz run -a history_ops corpus/history_ops -- \
  -fork=8 -ignore_crashes=1 -max_total_time=600 -max_len=4096
```

Reproduce or minimize one input:

```sh
cargo +nightly fuzz run -a history_ops regressions/history_ops/<file>
cargo +nightly fuzz tmin -a history_ops artifacts/history_ops/<crash>
```

`STOCKFOIL_FUZZ_TRACE=1` makes `history_ops` print each operation and both
histories, and `admission_bytes` print one `TRACE <mode> <kind> <outcome>
<validity>` line per input (handy for corpus coverage summaries:
`... | grep ^TRACE | sort | uniq -c`).

## Regressions

Minimized inputs for bugs the fuzzers found, both fixed:

- `regressions/history_ops/liveness_min`: a draft signed by its own author,
  with admitted parents, was never admitted when its canonical fragment
  contained a commit whose covering signature lay outside the fragment. Such a
  draft is now certified as a standalone commit.
- `regressions/admission_bytes/read_size_overflow`: a `read_size` near
  `usize::MAX` overflowed the header buffer bound. Sizes are now clamped and the
  bound saturates.

`corpus/`, `artifacts/`, and `target/` are git-ignored; regenerate seeds with
`gen_seeds`.
