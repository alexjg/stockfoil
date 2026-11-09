# Verifying the current Stockfoil with Verus

For continuation in a new thread, start with [HANDOFF.md](HANDOFF.md).

This work is rebased onto `main` at
`f2d470eeae4d11fe7bd7d7f65d9d88bcd8f0a65d`. The actual public captured ancestry
API is now verified, including construction, membership, heads, clone, causal
difference iteration and error mapping. See [API_TARGET.md](API_TARGET.md) for its precise scope. **It does not verify the full production
crate.** Shared executable proofs and conditional model theorems are listed
separately below.

**Rebase scope:** see [REBASE_STATUS.md](REBASE_STATUS.md). The rewritten
production traversal is lazy and ordered by ready commit hashes. The old eager
candidate assembler is now verification-only reference code; its proof does not
verify that traversal. Public materialization now runs through `Materialization`
and `Ledger`; raw History change methods are internal (or fuzzing-only).

Work now continues on a single branch beyond the [verified ancestry API](API_TARGET.md).
Intermediate layers no longer receive separate PRs.

The authorization strategy is tracked in the [authorized visibility plan](AUTHORIZED_VISIBILITY_PLAN.md),
which states the public contract, proof dependencies and completion criteria.
The [remaining public API work](PUBLIC_API_PROOF_BOUNDARY.md) maps all five requested
guarantees to concrete boundaries and records the accepted standard-library boundary.
[TRUSTED_STDLIB.md](TRUSTED_STDLIB.md) enumerates the 38 enabled Rust/stdlib contracts/declarations;
[TRUSTED_BLAKE3.md](TRUSTED_BLAKE3.md) enumerates the five hashing assumptions.

## Public partial resume, planning and acknowledgement replay

The reconstruction relation now reaches actual public API calls. A checked caller
serializes a partial checkpoint, decodes it, resumes two `Materialization`s against
independently rebuilt `Ledger`s, calls `plan` with the same positive budget, and
acknowledges both batches through `complete`. With no new rejection or payload
request in that acknowledgement, the two runs publish equivalent checkpoints,
equal counts and identical checkpoint bytes. Existing direct and cascading
rejections are allowed and remain effective.

The premise is explicit: the histories contain the same commit hashes, parent-edge
sets and authors; the saved checkpoint has known binding references and a valid
captured frontier/count. These are data-validity conditions, not an assumption
that planning succeeds. The caller proves success from the actual planner's
error contracts. Current ledger policies and visibility-change support may differ:
the partial traversal uses its saved target and permissions before any new view.
Node indices, chain assignments, History identities and payload handles may differ.

Batch comparison preserves event order and kind, hashes, authors, parent
**membership**, and activation/retraction decisions. Each delivered payload handle
is separately proved to belong to that hash in its own ledger. Parent-vector order
and multiplicity are not equated by the graph relation; nor are process-local
payload handles or batch tokens. The library still returns each history's stored
parent vector. A concrete checked fork/join consumer reverses arrivals and parent
order, changes payload handles and current ledger policy, and exercises the public
codec/resume/plan/complete path. Separate comparison witnesses ensure author,
parent membership, visibility and event order have not been discarded by the spec.

[replay_plan.rs](replay_plan.rs) composes source selection, skipped-prefix uniqueness,
record mapping, rejection filtering, event translation, counts and pending
checkpoints. [replay_ack.rs](replay_ack.rs) proves unchanged acknowledgement cannot
introduce a correction for an already-filtered active delivery, including when
other commits have permanent rejections. The planner contract now also retains
its captured before/delivered sets; a checked topology lemma exposes parent edges
and their hash/index mapping. These are proof changes with no runtime or wire-format
change and no new trusted declarations.

**Remaining reconstruction work:** compare feedback that adds rejections or asks
for payloads and the resulting correction rounds; compose repeated live driving
through completion of the saved capture; allow later history additions outside that
capture; and handle the subsequent fresh view. This milestone does not equate
application state or establish external atomic persistence, admission, compaction,
sync minimality or attestation completion.

Validation: **1,053 full-suite checks** and the overlapping **212 strict checks**
pass, along with **266 workspace tests**, **76 trust-audit regressions**, all-features
compilation, rustdoc links and formatting. All **18 new semantic mutations** reject
with proof diagnostics; all **1,124 configured anchors** match. The complete
mutation sweep was not rerun. The 43 project library contracts and two existing
ignored tests are unchanged.

## Replay across reconstructed histories

The actual source constructor, advancement and checkpoint methods now have checked
relational consumers for independently rebuilt histories. The premise describes
the data being reconstructed: the same commit hashes, direct parent-edge sets and
commit authors. It does **not** require matching node indices, chain assignments,
History identities, caches or caller-assigned payload handles. Both values use the
ordinary maintained Topology invariant; no new trusted declaration is introduced.

[history_equivalence.rs](history_equivalence.rs) derives reachability from this
explicit hash-based graph isomorphism by induction over parent paths. It then
proves equal causal selections and authorization sets for the saved heads and
permissions, including bounded permissions with unknown or out-of-view frontier
members. The canonical minimum-ready-hash order is consequently the same. These
conclusions are proved from the parent edges and authors, not supplied as replay
assumptions.

[replay_examples.rs](replay_examples.rs) calls the production methods. It proves
that reconstructing a source skips the same saved prefix and leaves the same
remaining selection, and that excessive counts fail as `Malformed`. A repeatable
paired advancement theorem preserves corresponding progress under any common
budget, including zero, reset-slot reservation and exhaustion. Records retain the
same hash, new-delivery flag and visibility; each payload handle resolves to that
hash in its own History. Raw handles may differ. Corresponding sources produce
semantically equal checkpoints and identical serialized bytes.

The persisted-partial consumer advances a live source, prepares and serializes
its checkpoint through the public codec, decodes it, reconstructs the source in
the other History, and proves that its next records and checkpoint agree with
uninterrupted execution. A concrete fork/join consumer constructs both topologies
through actual insertions, reverses root arrival and join-parent order, changes
payload handles, and establishes the equivalence premise without assuming it.
Existing runtime regressions already exercise reordered rebuilds and remapped
payload handles, including later deliveries.

**Scope:** this closes the source-level reconstruction and graph-isomorphism gap
for histories containing the same logical commits. The next step is composition
through public `Materialization::resume`/`plan`, event translation and completion,
and extending the relation to later additions outside the saved capture. Those
end-to-end and growth claims are not established here. The reconstruction premise
is a comparison of stored graph data; this theorem does not prove admission or
that a persistence implementation rebuilt those data faithfully. Application
semantics and atomic external persistence remain caller obligations. Production
runtime behavior and the checkpoint wire format are unchanged.

Validation: **1,036 full-suite checks** and the overlapping **212 strict checks**
pass, along with **266 workspace tests**, **75 trust-audit regressions**, all-features
compilation, rustdoc links and formatting. All **12 new semantic mutations** reject
with proof diagnostics; all **1,106 configured anchors** match. The complete
mutation sweep was not rerun. The 43 project library contracts and two existing
ignored tests are unchanged.

## Public checkpoint serialization is verified

The actual public `Checkpoint::to_bytes` and `Checkpoint::from_bytes` bodies are
now checked in `verified/checkpoint_codec.rs`. For every typed checkpoint,
encoding and then decoding preserves the complete metadata: follow rule and
ordered pin, completed cursor and permissions, rejected hashes and cascade flags,
and the presence and contents of partial progress (target, captured delivery heads
and exact `u64` processed count). There is no history-validity precondition.

The decoder agrees with an explicit structural parsing model on every input. It
rejects unknown versions/tags, truncated or oversized declared collections,
duplicate rejected hashes/authors, invalid booleans, malformed nested cursors and
trailing bytes. Unique map entries may arrive unsorted, preserving the existing
format's acceptance rules. Encoding is deterministic for equal checkpoint values,
including maps built in different insertion orders. The rejected map still uses
its existing BTreeMap iteration; no additional sorting or project trust contracts
were introduced.

[checkpoint_codec_examples.rs](checkpoint_codec_examples.rs) calls the actual
public methods to establish round trips, deterministic output and exact input
classification. Its serialize/decode/resume caller proves that
`Materialization::resume` receives the same complete checkpoint and establishes
the existing source/checkpoint/counter invariants. This is metadata preservation,
not a theorem about reconstructed-history replay or application state persistence.
Compatibility tests compare with the previous outer format and exercise all
follow/progress variants, maximum processed counts, unsorted rejected entries,
duplicate hashes, invalid flags, truncation and trailing bytes.

**The checkpoint/cursor codec gap is closed.** The next library obligation is
replay equivalence after history reconstruction or capture recompilation,
including canonical remaining records and their metadata. Semantic head/progress
validation remains in the planner. Application payload semantics and external
atomic persistence remain caller obligations; compaction, sync minimality,
admission and attestation completion remain separate verification goals.

Validation: **1,019 full-suite checks** and the overlapping **212 strict checks**
pass, along with **266 workspace tests**, **74 trust-audit regressions**, all-features
compilation, rustdoc links and formatting. All **24 new semantic mutations** reject
with proof diagnostics; all **1,094 configured anchors** match. The complete
mutation sweep was not rerun. The 43 project library contracts and two existing
ignored tests are unchanged.

## Complete cursor and permission-map codecs are checked

The actual `MaterializationCursor::to_bytes` and `from_bytes` bodies now run in
the verification harness, along with the complete view and permission-map codecs.
Encoding followed by decoding preserves delivery heads, target heads, every
listed author's rule and the rule for unlisted authors. This holds for arbitrary
typed cursors, without a history-validity premise. Head/frontier order and repeated
hashes are retained exactly.

The decoder agrees with an explicit structural parsing specification on every
input. It rejects unknown versions and tags, truncated collections, duplicate
authors and trailing bytes. Unique author entries may arrive in any order, as in
the prior decoder. Encoding is deterministic for semantically equal cursors:
author keys are emitted in byte order, using the existing approved standard
sorting contract. There are no new project trust declarations or format changes.

[The checked cursor callers](cursor_codec_examples.rs) exercise the actual methods
for arbitrary round trips, deterministic output and input classification. The map
caller also proves that decoding leaves an arbitrary following suffix untouched.
Compatibility tests compare against the previous format, including all permission
variants, repeated frontiers and noncanonical author order.

**API scope:** `MaterializationCursor` is internal in normal builds and exported
under `fuzzing`. `Checkpoint` uses this codec internally. The public
`Checkpoint::to_bytes`/`from_bytes` composition proof followed this milestone and
is now complete (see the public checkpoint section). Reconstructed-history replay
remains subsequent work.

Validation: **982 full-suite checks** and the overlapping **212 strict checks**
pass, along with **264 workspace tests**, **72 trust-audit regressions**, all-features
compilation, rustdoc links and formatting. All **20 new semantic mutations** reject
with proof diagnostics; all **1,070 configured anchors** match. The complete
mutation sweep was not rerun. The 43 project library contracts and two existing
ignored tests are unchanged.

## Checked checkpoint codec primitives

The production byte reader now lives in `verified/reader.rs`; all existing wire,
cursor, export and checkpoint callers use that same implementation. Its contracts
establish exact prefix consumption, unchanged input on short fixed-width reads,
collection counts bounded by the remaining bytes before iteration, and rejection
of trailing data. Hash-list decoding preserves order and duplicates and consumes
exactly the length prefix and the declared members.

`verified/metadata_encoding.rs` checks little-endian integer and ordered hash-list
encoding, including preservation of an existing output prefix. The production
permission encoder/decoder and checkpoint boolean decoder now live in
`verified/permission_codec.rs`. Permission parsing preserves the exact frontier
and distinguishes denied, unbounded and bounded rules; unknown tags and truncated
frontiers fail. Boolean parsing accepts only zero and one. These are structural
checks: named hashes need not exist in a history.

[codec_examples.rs](codec_examples.rs) calls those actual bodies for arbitrary
integer, hash-list and permission round trips, preserving an arbitrary unread
suffix, and for malformed-length/tag/trailing-byte rejection. Integer conversion
uses checked copies and byte shifts so this adds no project trust declarations.
The wire format and public API are unchanged. Native little-endian compatibility
and truncated-input regressions supplement the proofs.

**Remaining codec work:** the complete `Checkpoint::to_bytes`/`from_bytes` and
`MaterializationCursor` methods are not yet proved. They still need composition
through magic/version checks, sorted permission/rejection maps, duplicate-key
rejection, nested cursor lengths, follow rules and optional captured progress.
The reader and leaf codecs are checked prerequisites, not a full checkpoint
round-trip theorem. Reconstructed-history replay remains the following step.

Validation: **934 full-suite checks** and the overlapping **212 strict checks**
pass, alongside **262 workspace tests**, **70 trust-audit regressions**, all-features
compilation, rustdoc links and formatting. All **22 new semantic mutations** reject
with proof diagnostics; all **1,050 configured anchors** match. The full mutation
sweep was not rerun. The 43 project library contracts and two existing ignored
tests are unchanged.

## Batch counters now remain exact through correction rounds

The actual pending-batch constructor retains the initial counts and active-delivery
sequence as erased proof data. Checked translation establishes distinct delivered
hashes, unique active deliveries contained in that set, and at most one counted
delivery/activation/retraction event per captured source record. The proof data
adds no runtime storage or work.

The new `counters_consistent` invariant is established by both driver constructors
and preserved by every public driver mutation, including errors and callback
rollback. Within one initial batch and its follow-up rounds:

- Delivery and activation counts stay equal to the initial source-batch counts.
- Retractions equal the initial retraction count plus the cardinality of the
  remembered correction set. That set grows monotonically within the original
  active-delivery set; each round adds exactly its previously uncounted hashes.
- A reset preserves the original accounting, clears active correction work and
  sets the sticky rebuild flag. Repeated payload requests do not inflate counts.
- The prior distinct-rejection-count and checkpoint-progress invariants continue
  to hold.

Successful public `complete` now explicitly preserves the initial delivery and
activation totals and bounds additional retractions by the original active-delivery
set. The combined delivery/activation/retraction count is bounded by captured
processed records plus that correction budget. The existing `step`/`update`
trace and summation proofs remain in force; totals describe emitted events,
not globally distinct commits across resets or later materialization batches.

[counter_examples.rs](counter_examples.rs) checks actual public completion,
exact correction deltas, correction bounds and unchanged retraction counts when
no new correction is remembered. [lifecycle_examples.rs](lifecycle_examples.rs)
now preserves the stronger invariant across arbitrary public call sequences.
No executable behavior, caller precondition or trust assumption changed.

The pending-batch counter relationship gap is closed for normal-return API
sequences. Finite counter exhaustion remains a documented error. The next library
work is checkpoint codecs and replay equivalence after history reconstruction or
capture recompilation; admission, compaction, sync minimality and attestation
completion remain separate goals. Application semantics and external atomic
persistence remain caller obligations.

Validation: 902 full-suite checks and the overlapping 212-check strict subset
pass, along with 260 workspace tests, 68 trust-audit regressions, all-features
compilation, rustdoc links and formatting. All 21 new semantic mutations and one
updated constructor mutation reject with proof diagnostics; all 1,028 configured
anchors match. The complete mutation sweep was not rerun locally. The two
existing ignored tests are unchanged.

## Pending checkpoints and rejection counts now compose across calls

A new inductive `checkpoint_consistent` contract extends source lifecycle
consistency through every public driver mutation. Retained sources conserve
`processed + remaining = captured total`. Each pending checkpoint records the
source's exact captured target and delivery frontier: normal partial work saves
the processed-record count, finished work saves the completed cursor, and a reset
saves an empty cursor with zero processed records for replay. Pending progress
cannot exceed the captured total. A changed follow rule does not retarget an
already captured traversal.

Across arbitrary feedback rounds, pending rejection sets retain all published
rejection hashes, and the pending rejection count equals the number of distinct
hashes added since that published checkpoint. Repeating a rejection does not
inflate the count. Successful `complete` now explicitly guarantees that its
published checkpoint matches the retained source (or its reset) and that its
reported rejection count equals the newly published set difference. Follow-ups
and errors do not publish that staged checkpoint.

The proof covers callback failures and checkpoint restoration: while pending work
remains, the driving helpers retain the original published checkpoint, so restoring
it does not invalidate the pending rejection-count relationship. Constructors
establish the invariant, and actual `set_follow`, `plan`, `complete`, `mark_local`,
`step`, `update` and `change` bodies preserve it without new caller preconditions.
[lifecycle_examples.rs](lifecycle_examples.rs) composes those public calls;
[pending_checkpoint_examples.rs](pending_checkpoint_examples.rs) checks exact
progress, reset handling, bounded progress, completion counts and repeat reports.
The set-cardinality and correction-round lemmas are checked production proof code
in [materialization_invariants.rs](../stockfoil/src/verified/materialization_invariants.rs).

This invariant concerns retained sources and pending work. It does not claim that
an arbitrary checkpoint supplied to `resume` has valid references or a valid skip
count: those still receive the existing checked validation errors when used.
Remaining library work includes stronger cumulative delivery/activation/retraction
relationships, checkpoint codecs and replay equivalence after history
reconstruction. Application interpretation, local edit/byte agreement and atomic
external persistence remain caller obligations. No executable behavior or trust
contract changed; the boundary remains 38 Rust/stdlib plus five BLAKE3 declarations.

Validation: 894 full-suite checks and the overlapping 212-check strict subset
pass, along with 260 workspace tests, 67 trust-audit regressions, all-features
compilation, rustdoc links and formatting. All 21 new semantic mutations and nine
updated source-lifecycle mutations reject with proof diagnostics; all 1,007
configured anchors match. The complete mutation sweep was not rerun locally.
The two existing ignored tests are unchanged.

## Source lifecycle invariant now spans the public driver API

`Materialization::new` and `resume` establish a checked source invariant:
any retained source is ready and belongs to the driver's bound History, and any
outstanding batch has a retained source. Every public driver mutation preserves
it on normal return: `set_follow`, `plan`, `complete`, `mark_local`, `step`,
`update` and `change`. The proof includes errors, callback failures, follow-up
loops, checkpoint restoration and local publication; no caller-validity
precondition was added to those APIs.

Consequently, for states reached through these APIs, `plan` and `complete` return
`Error::Poisoned` only when the driver was already poisoned. Their defensive
missing/unready-source paths are unreachable on normal-return call sequences.
A planning `WrongHistory` error requires an actual mismatch with the driver's
bound history; a retained source cannot introduce a different owner. Genuine
callback poisoning, stale tokens, invalid checkpoints/reports and finite-counter
exhaustion remain possible and retain their checked behavior.

[lifecycle_examples.rs](lifecycle_examples.rs) checks arbitrary finite sequences
of actual public calls, including supplied tokens/reports, retained tokens from
planning and follow-ups, local acknowledgement, step/update/edit, and resume from
arbitrary checkpoints. Separate callers cover correction chains and constructor
establishment. This is an inductive source invariant, not an assertion that all
error paths or all cross-field inconsistencies are impossible. Stronger
checkpoint/pending/count relationships, checkpoint codecs and replay equivalence
across reconstructed histories remain library-side work.

Application payload interpretation, agreement between local state edits and their
encoded bytes, and atomic persistence of application state with its checkpoint
are **caller obligations**. They are not unfinished proofs of Stockfoil. A concrete
verified application may demonstrate the interface, but is not required to close
the library contract. Stockfoil's own checkpoint encoding/decoding and resume
behavior remain its responsibility.

No executable behavior or trust contract changed. The boundary remains 38
Rust/stdlib contracts plus five approved BLAKE3 declarations. These are guarantees
on normal return; this invariant does not cover resuming an unwound driver.

Validation: 887 full-suite checks and the overlapping 212-check strict subset
pass, along with 260 workspace tests, 66 trust-audit regressions, all-features
compilation, rustdoc links and formatting. All 19 new semantic mutations reject
with proof diagnostics; all 986 configured anchors match. The complete mutation
sweep was not rerun locally. The two existing ignored tests are unchanged.

## Public commit construction and local change now verify

The actual public History/Ledger `begin_commit`, streaming `CommitBuilder`
clone/update/finish, `Document::commit` and `Materialization::change` now have checked
bodies. Hash-input construction preserves the bijou64 parent count, ordered parents,
author and payload digest. Change connects the actual edit callback and its bytes
to publication and acknowledgement; returning a Draft leaves the driver current
and dirty. Errors retain the established preflight, mutation and poisoning rules.

The user approved trusting BLAKE3's implementation. The explicit boundary is now
**38 Rust/stdlib contracts plus 5 BLAKE3 declarations**; see
[TRUSTED_BLAKE3.md](TRUSTED_BLAKE3.md). No collision resistance or fresh hash is
assumed. Concrete application-state/checkpoint correspondence, durable storage,
general lifecycle reachability, compaction, sync minimality and attestation
completion remain open. See [API_TARGET.md](API_TARGET.md#public-commit-construction-and-local-change-now-verify).

The commit-construction milestone rejected all 33 of its new semantic mutations;
its configured suite then contained 967 cases. Its complete sweep was not rerun locally.

## Commit preparation now preserves checked metadata

The sections below record preceding milestones. Their historical validation and
trust counts describe those stages; the active boundary is the commit/change
composition above.

The actual preparation helpers select and validate parents before constructing a
builder. Explicit parents retain order and repetitions; None selects every
current delivered head, sorted without duplicates. Explicit empty parents remain
empty. Default selection cannot fail with a missing parent. The metadata retained
by the actual builder has checked clone/accessors and a checked conversion to
`Prepared`, preserving author and parents for any supplied digest.

Public `Prepared::signing_bytes` is now checked against the current public hash.
Builder hashing and the public commit/change wrappers are checked by the newer
composition above, under the approved BLAKE3 boundary.
See [API_TARGET.md](API_TARGET.md#commit-preparation-now-preserves-checked-metadata).

## Local edit phases now check callback effects and poisoning

The actual helpers called by `Materialization::change` now check preflight order,
callback invocation and exact returned bytes/errors, followed by post-publication
acknowledgement. Preflight rejects preserve application state and do not newly
poison the driver. Callback errors and later publication/acknowledgement errors
poison it; no application rollback is claimed. Returning `None` does not publish,
but does not imply that the callback left application state unchanged.

Callback effects use the native supplied closure's contract, with no application
axiom. The shared checked parent-validation loop now also serves `begin_commit`.
The complete public wrapper and commit preparation now verify as described above.
Concrete application-state/checkpoint correspondence remains unverified.
See [API_TARGET.md](API_TARGET.md#local-edit-phases-now-check-callback-effects-and-poisoning).

## Public prepared-commit publication is now checked

The actual `History::publish_commit` and `Ledger::commit` bodies now verify exact
stored metadata, ordered/repeated parents, payload IDs, draft registration and
preservation of earlier history. Duplicate hashes keep the original record and
payload. Wrong-author, first-missing-parent and counter/capacity errors have checked
precedence and preserve the History/Ledger. Publication now returns explicit
exhaustion errors instead of panicking on those counters.

The actual Document helper after hashing also verifies first-write payload storage,
including retained bytes when Ledger publication fails. A successful proof composes
public commit, local acknowledgement and checkpointing. `Prepared` carries no
assumed authenticity invariant: hash construction and both public wrappers are now checked above; concrete
state correspondence and persistence remain open.
See [API_TARGET.md](API_TARGET.md#public-prepared-commit-publication-is-now-checked).

## Public `mark_local` now checks local acknowledgement

The actual public method checks exact stored draft metadata, parent-head sets,
unchanged policy, a sole new delivery, authorization and rejection. Success moves
the checkpoint to the current view, advances a pin and marks it dirty without
applying events. Errors preserve the driver apart from initial history binding.
Public History/Ledger `commits_since` and Ledger metadata/ancestry queries are now
checked too. A nonempty proof composes acknowledgement with a real checkpoint
callback; runtime tests compose it with update without repeated delivery.
See [API_TARGET.md](API_TARGET.md#public-mark_local-now-checks-local-acknowledgement).
The complete public `change` wrapper and preparation are now checked above. The caller's
claim that application state already reflects a draft, concrete state/checkpoint
correspondence and durable storage remain outside these guarantees.

## Public `update` now composes verified steps

The actual public update loop now connects every completed step and application
state transition to its result. Successful reports have exact cumulative counts,
sticky rebuild status and initial/final completed heads; the driver is current
and clean. A later step failure retains earlier successful checkpoints. Report
counter overflow returns an explicit exhaustion error after the successful step,
preserving its checkpoint without newly poisoning the driver. The loop has a
checked decreasing ticket measure; application callback termination and durable
state correctness remain outside the proof.
See [API_TARGET.md](API_TARGET.md#public-update-now-composes-verified-steps).
The next boundaries are lifecycle reachability and concrete
application-state/checkpoint correspondence. The current boundary is 38 Rust/stdlib contracts plus 5 BLAKE3 declarations.

## Public `step` now checks the in-memory driving protocol

The actual public method now composes checked planning, byte resolution, application
calls, feedback completion, and checkpointing. Its contract preserves callback
arguments/errors, bounds every activation by the first plan's captured authorization,
and distinguishes preflight errors from later checkpoint restoration and poisoning.
Callback effects come from the actual method's contract; no application or storage
correctness is assumed. This is a normal-return guarantee, not application rollback,
panic recovery, or callback termination. The driver loop itself has a checked
decreasing ticket measure.
See [API_TARGET.md](API_TARGET.md#public-step-now-checks-the-in-memory-driving-protocol).
Public `update`, its cumulative counters, `mark_local` and `change` are now checked
above. General lifecycle reachability and concrete application/durability proofs
remain unfinished.

## Payload resolution now preserves the verified event envelope

Public `Event::map_payload`, `try_map_payload` and `Batch::resolve` now preserve
exact event metadata, visibility and order under the supplied callback contract.
The actual Document storage/accessors and internal byte resolver are checked too;
`step` uses that resolver before calling the application. Missing storage fails the
whole resolution, and large IDs are checked before narrowing to `usize`.
The planning-to-bytes proof preserves captured authorization and rejection filtering.
See [API_TARGET.md](API_TARGET.md#payload-resolution-now-preserves-the-verified-event-envelope)
for contracts, witnesses and limitations. `step` and `update` now compose these calls as described above; arbitrary application
side effects still need application-specific proofs; the current library boundary is documented above.

## Public completion is now checked

The actual public `Materialization::complete` body now runs in the harness with
**no caller validity preconditions**. Together with `plan` and `is_current`, this
checks the state-independent batch/feedback protocol through checkpoint publication.
It does not prove application effects or persistence of that checkpoint.

After history binding and report validation, completion copies the latest follow
rule, merges rejection hashes/cascade flags, and adds exactly the number of newly
recorded hashes. Correction selection uses that updated rejection map. Retracted
counts increase by exactly the generated correction count. Repeating an already
recorded rejection adds zero, including across immediate follow-up rounds.
Delivered/activated counts remain those of the original source batch.

Follow-up events are precisely the selected retractions and requested stored
payload IDs, or Reset alone when visibility changes are unsupported. They contain
no activation events. The fresh token holds the updated pending checkpoint and
counts; the published checkpoint and last completed counts remain unchanged.
An unsupported cascade stages the original source target/frontier at progress zero,
clears active deliveries, and retains rejections and the restart flag. When no
follow-up events remain, completion publishes that pending checkpoint and its exact
counts together, clearing the source precisely when restart was staged. Follow
changes are preserved without replacing an interrupted traversal's captured target.
Payload requests still permit any known hash; this is not payload confidentiality.

The error contract separates retryable validation failures from failures after
acceptance. Poisoning/history/reference checks precede token validation, which
precedes rejection validation and unknown payload requests. Guard failures preserve
the entire driver; invalid token/report errors preserve pending work, apart from
any newly established history binding. After report acceptance, a failure consumes
the token while preserving the published checkpoint, last counts and source.
Restoring application state and checkpoint together is required before continuing
from those later failures. Successful completion also makes the submitted token
and its clones stale; follow-ups receive a strictly later ticket.

Two production failure paths are now explicit: overflowing rejection/retraction
counts return `Error::Exhausted`, and a requested reset with no stored source returns
`Error::Poisoned` instead of an unchecked unwrap. Existing poisoned-source and ticket
exhaustion errors retain their ordering. The contracts identify the exact arithmetic,
source or ticket condition for those failures; they do not assume those conditions
are unreachable. Proving their absence for every reachable API interleaving remains
an additional lifecycle invariant obligation. There are no new application or
library assumptions.

[complete_examples.rs](complete_examples.rs) checks arbitrary public outcomes,
invalid-report token preservation, poisoning precedence, repeated-rejection counts,
and successful acknowledgement when no rejections need correction. The latter
requires no unused ticket or source: even maximum counters accept zero increments.
The nonempty [plan_examples.rs](plan_examples.rs) witness now calls `plan`, proves
an actual active delivery, calls `complete`, verifies the published heads/counts,
and proves `is_current` returns true. Its History comes from checked insertion;
this still does not prove public commit admission. Runtime regressions exercise
multiple feedback rounds and deliberately injected extreme internal counter/source
states without enormous allocations.

Remaining: concrete application effects and matching application-state/checkpoint persistence,
`change` and commit preparation, and general lifecycle reachability/termination properties.
Compaction, sync minimality, attestation completion, and replay equivalence across
independently reconstructed graphs remain separate open goals.

## Public planning is now checked

The actual public `Materialization::plan` and `is_current` bodies are in the
harness, with **no caller validity preconditions**. The proof includes the actual
`Ledger::history`, `permissions` and `view` accessors. Ledger's storage is generic
in its untouched receive-options and queue fields, with the existing concrete
production types as defaults. The same executable methods are checked for all
values of those fields; there is no replacement Ledger or assumed accessor.
Ledger construction, admission and queue behavior are not thereby verified.

`is_current` returns true exactly when the driver is unpoisoned, has no pending
batch or partial checkpoint, has no foreign binding, and its completed delivery
and view match the ledger and follow rule. Equality is structural: explicit
policy entries, fallback rules, bounded-frontier order/multiplicity and pinned
head order/multiplicity all matter. Working/signed heads retain the existing
BTree/Ord model. Actual permission, view and cursor equality bodies are checked.

`plan` preserves the published checkpoint, dirty flag, last completed counts and
visibility capability on every returned outcome. A successful batch has a fresh
accepted token and at most the supplied budget of events, including Reset. Its
records are the exact canonical prefix of the retained or newly selected source;
progress counts and remaining selection agree. The event sequence is the exact
translation of those records. Every activation is in the source's captured
authorized set and passes the actual direct/cascade rejection query. Pending
checkpoint metadata, event counts, delivered hashes and active-delivery order
match that batch. `Ok(None)` means the driver is current and has no source.
An empty `Some(batch)` remains meaningful: it can stage a view/policy change
without application events, and still needs acknowledgment.

The proof checks error precedence and origins: poisoning/history/reference checks
precede zero budget, which precedes pending-work rejection. Guard failures leave
the whole driver unchanged. Budget/pending errors retain any successful initial
history binding. Startup errors can leave an old finished source discarded; ticket
exhaustion can occur after source advancement. Every returned error preserves the
Protocol and its pending token, and never publishes a checkpoint. This is not a
whole-method rollback guarantee.

[plan_examples.rs](plan_examples.rs) calls the actual public methods with arbitrary
Ledger field types, checks the returned authorization/budget/frame contracts, and
constructs a nonempty History through checked insertion to prove an actual active
delivery. This fixture does not prove public commit admission. Runtime regressions
cover structural policy changes, reordered/repeated pins, empty checkpoint batches
and concrete-Ledger function-pointer compatibility.

Public `complete` and per-source feedback counters are now checked as described above.
Public `update` and aggregate progress counters are checked above; matching
application-state/checkpoint persistence remains open. This milestone proves planning
and event authorization, not the effects of executing those events. Compaction,
sync minimality and attestation completion remain separate goals. The trusted
boundary at that stage contained 37 library/Rust contracts, with no application assumptions.

## Run

Install [Verus 0.2026.11.02.8bf68ee](https://github.com/verus-lang/verus/releases/tag/release/0.2026.11.02.8bf68ee)
and its matching Rust toolchain (`1.99.1` for Linux x86-64). The archive includes
Z3 and vstd.

```sh
rustup toolchain install 1.99.1 --profile minimal
export VERUS_BIN=/absolute/path/to/verus-x86-linux/verus
bash verification/verify.sh
python3 verification/check_mutations.py
cargo +1.99.1 test --workspace --locked
```

`VERUS_BIN` defaults to `verus` on PATH. The runner checks the exact version,
runs the original suite with `--no-cheating`, then the full suite with the
enumerated standard-library and BLAKE3 contracts. The mutation runner requires both baseline
passes, then uses `--mutation-only` to run the complete expanded suite and audit
on each temporary mutation without repeating the subset pass.
Compilation or tool setup failures do not count as mutation rejection.

The original-suite baseline reports **212 verified, 0 errors**; the full suite
reports **1,053 verified, 0 errors**, conditional on **38 Rust/stdlib contracts
and five approved BLAKE3 declarations**. These overlapping proof counts are not additive. The mutation suite
contains 1,124 deliberate errors. The authorization plan records which stage these
results establish; owned traversal stability, actual materialization planning,
record mapping, batching, immutable public batch tokens and their pending-work
handshake, durable checkpoint values/transitions, and the captured source's resume/progress
lifecycle, record-to-event translation, cascade filtering, event summaries, report admission, rejection recording, correction generation, captured restart, public Materialization value methods, its history-binding guard, source-input selection, reset decision and source installation are checked. Public `plan`, `complete` and `is_current` are checked too, as are Event/Batch payload mapping and actual Document byte resolution. Public `step` and `update` compose actual callbacks, completion, error recovery and
exact aggregate counters. Public commit preparation, streaming hashing-input
construction, `Document::commit` and `Materialization::change` now compose those
checks; matching application-state/checkpoint persistence remains unfinished.

[CI](../.github/workflows/verus.yml) pins the Verus archive and SHA-256, runs the
proof and mutation suites, and tests the Rust workspace. Normal Cargo builds
erase proofs using the pinned `verus_builtin_macros` procedural macro. They do
not need the Verus executable or a runtime vstd dependency. This workspace uses
`std`; this milestone makes no `no_std` compatibility claim.

## Source selection, visibility reset and installation

The production follow-target helper now selects the exact stored delivered heads
for Working, stored admitted heads for Signed, or the exact caller pin (including
order and duplicates), and copies the supplied policy. Collected head sets are
unique and increasing under the existing vstd BTree/Ord model. This does not add an
authentication invariant for admitted heads or a proof of the derived bytewise Ord
implementation. `Ledger::view` and source startup use this same checked helper.

The actual visibility-change decision proves `before != after ∩ delivered`, where
all three sets are interpreted against the supplied History. Thus new deliveries
alone do not force reset, while changes to previously delivered participation do.
The formula deliberately leaves `before` unmasked: arbitrary stored cursors may
name a prior view outside their prior delivery. Unknown heads return an actual
missing requested hash. Permission proofs now establish that each clock endpoint
is a real node on its coordinate's chain. The production clock equality iterator
is checked, including its length comparison; for valid clocks it compares exact
maps independently of tree shape. These facts make map equality equivalent to
membership equality without adding a trusted application fact or changing the
linear comparison algorithm.

For interrupted traversals, input selection retains the saved base, target policy,
delivery frontier and processed count, with no second reset. Current follow rules,
policy and later deliveries do not replace that capture. Fresh traversals use the
current follow target and delivery frontier, start at zero, and use an empty base
exactly when visibility changes are unsupported and the comparison requires reset.

The actual source-installation helper composes these inputs with `Source::new`.
Success installs precisely that validated capture and its canonical remaining
selection, changing no other Materialization field. Errors are either an input
validation failure or a constructor error for those selected inputs; the entire
previous Materialization, including any previous source, remains unchanged. A
checked consumer connects startup to source emission, authorized event translation,
checkpoint staging and fresh token issuance. Nonempty witnesses cover reset,
new-delivery-only changes, equal clock maps with different insertion orders, and
successful partial startup; invalid saved counts preserve the previous source.

Public planning and Ledger argument extraction are now checked as described in
"Public planning is now checked". Public completion is now checked too. The remaining
boundary is matching application-state/checkpoint persistence, local edits,
and general lifecycle reachability. Driver loop progress is now checked. Helper error guarantees do not imply
rollback of earlier outer-method transitions. These are normal-return guarantees;
application behavior, panic recovery and replay equivalence across independently
reconstructed graphs or recompiled captures remain open.

Validation for source startup: 209 strict/701 full checks, 220 workspace tests
(two existing ignored doctests), 40 audit regressions, default/all-features builds,
formatting and rustdoc links. All 34 new semantic mutations reject with proof
diagnostics; all 661 anchors match. The complete sweep was not rerun. The trusted
boundary remains 37 library/Rust contracts.

## Public Materialization values and history binding

The harness now contains the actual exported `Materialization` type. Its public
`new`, `resume`, `checkpoint`, `follow`, `set_follow`, `heads` and `is_dirty` bodies
are checked without caller validity premises. Resume retains the exact supplied
checkpoint and visibility capability, while creating an unbound, clean, unpoisoned
driver with no source or pending work and zero counts. A fresh protocol cannot
accept an old batch token. Changing follow preserves the current capture, pending
work, completed cursor, rejections and partial progress; `heads` still names the
last completed view.

The production `check_history` guard used by planning, completion and the in-memory
driver is checked against the actual History identity accessor. Poisoning wins
over foreign-history errors; foreign-history errors win over missing references.
On initial binding, all completed delivery heads, completed view heads, pinned
heads and rejected hashes must exist. An unknown-reference error carries an actual
missing hash from those sources. Every returned guard error preserves the entire
Materialization value, including its owner and pending protocol; success changes
only an absent owner to the supplied History ID. Repeated checks on that bound
History preserve all state and do not revalidate changed checkpoint references.

Binding deliberately defers saved partial-traversal heads/counts to Source
construction. Successful binding can precede an error in the outer method: for
example, `plan` with zero budget retains its newly established owner. The guard's
unchanged-on-error guarantee must not be read as a transaction guarantee for the
whole public operation. Source selection, reset, completion/driver composition
and cumulative counters are now checked. Concrete application-state/checkpoint
correspondence and durable storage remain unverified.

Consumers prove arbitrary guard outcomes, retry after rejected binding, pending
token preservation under follow changes, independent missing-reference roles,
deferred partial validation, fresh-token isolation and a nonempty binding. Public
regressions cover retry, identical-but-foreign graphs, foreign completion without
token loss and error precedence. No application or library assumptions were added.

Validation for history binding: 209 strict/680 full checks, 217 workspace tests
(two existing ignored doctests), 37 audit regressions, default/all-features builds,
formatting and rustdoc links. All 30 new semantic mutations reject with proof
diagnostics, and all 627 anchors match; the complete sweep was not rerun. The
trusted boundary remains 37 library/Rust contracts.

## Correction rounds and captured restart

The production correction generator now proves the exact ordered sequence of
retractions followed by requested payloads. It retains active source order, skips
directly rejected hashes and remembered corrections, and selects every remaining
active delivery affected by a cascade. Actual source emission and translation
establish that the initial active-delivery list is unique; the composed consumer
therefore derives unique retractions. With unchanged History and checkpoint,
remembering the first round's corrections makes the next round's retraction list
empty. Later feedback may legitimately add new cascades.

Payload requests produce exactly the deduplicated wanted set, in the existing
vstd BTree/Ord model's order, with each actual stored payload ID. The report gate
supplies the known-hash premise. This does not add a proof of the derived bytewise
Ord implementation, nor restrict requests to the captured authorized view: any
known hash remains requestable, so this is not a payload-confidentiality guarantee.

If affected deliveries exist and visibility changes are unsupported, the generator
returns only Reset, suppresses payload requests, leaves the corrected set unchanged
and reports zero retractions. The actual Pending restart transition preserves
rejections, delivered/corrected sets, follow rule and numeric counts; it clears
active deliveries, sets restart/rebuilt and stages the original captured target and
delivery frontier at progress zero. A poisoned source returns an error with Pending
unchanged. This local error guarantee does not restore a token already consumed by
the outer completion method.

Checked consumers compose real source emission, event translation, report admission,
rejection recording, correction planning and nonempty follow-up token issuance. The
old token becomes stale and the new token carries the exact events and pending work.
A concrete two-node witness covers a nonempty cascade, stored payload and Reset.
Runtime tests cover direct rejection exclusion, deduplicated payload ordering,
repeated rounds, delayed checkpoint publication and captured replay after growth.

The history-binding guard and cumulative counter additions are checked above.
Concrete application-state/checkpoint correspondence remains unverified. These are
normal-return guarantees; allocation failure, unwind recovery and application
behavior are not proved. Compaction, sync minimality, attestation completion and
replay equivalence across reconstructed graphs remain separate open goals.

Validation for this milestone: 209 strict/655 full checks, 214 workspace tests
(two existing ignored doctests), 35 audit regressions, formatting, default/all-feature
compilation and rustdoc links. All 30 new semantic mutations reject with proof
diagnostics, and all 597 configured anchors match; the full sweep was not rerun.
The trusted boundary stays at 37 explicit library/Rust contracts.

## Public ancestry API

The actual exported `History` and `Ancestry` types are included in the harness.
`History::ancestry`, `Ancestry::contains`, `participates`, `heads` and clone are
checked without caller validity premises. The proofs establish exact frozen
parent-edge/policy membership, sorted unique requested heads (retaining redundant
ancestors), and exact `UnknownCommit`/`WrongHistory` errors. `History::new`, Default
and the public `is_ancestor` delegation are checked too. This is relative to the
stored graph, recorded authors and supplied permissions, not authenticated authors
or payload confidentiality.

The public proof uses the maintained topology, cache, permission and lineage
results below. It includes all actual auxiliary data fields and Error variants;
no stand-in History or trusted application accessor is used. The production
re-export wiring is pinned in `public_api_wiring.json` as a review guardrail.
Hash sorting/deduplication and Vec-to-Arc-slice conversion add two library
contracts. Arc slice access and clone use vstd directly. No application facts are
assumed. The capture/membership representation and public signatures are unchanged.

The new consumer proofs call the public methods with arbitrary borrowed arguments
and on a nonempty fixture. The fixture inserts nodes through the checked private
Topology path, not the public publication protocol. Fifteen new semantic mutations
are rejected with proof diagnostics. The audit's 40 regression tests include
redirected production exports, bypassed traversal helpers and changed index representation. The workspace tests
pass (220 tests, two existing ignored doctests), including normalization across every hash byte and public
capture/clone sharing. The full 661-case mutation sweep has not been rerun locally.

## Public causal difference iterator

The actual exported `Ancestry::difference` and `Commits` implementation are now
checked. Neither callers of `difference` nor callers holding an arbitrary borrowed
`Commits` supply validity premises. The successful iterator emits exactly
`after.causal() - before.causal()`, once per hash, in ancestor-first order and with
the smallest hash among currently eligible records. Both capture identities are
checked; an incompatible capture returns exactly `Error::WrongHistory`. Permissions
are deliberately ignored by this causal API, including denied commits and drafts.
Later history additions do not expand either capture.

The capture invariant now records that each causal coordinate's chain key equals
its stored tip's chain label. Checked construction and clone preserve that fact;
the lineage receipt and history-ID guard carry it into the current topology. The
public method derives the constructor's clock-ordering and chain-bound premises.
The actual shared `Traversal` wrapper is included in the harness, and its joint
invariant is maintained by the public `Commits` type invariant.

[`canonical_order.rs`](../stockfoil/src/verified/canonical_order.rs) proves a pure
sequence model from finite selected nodes, causal eligibility and bytewise hash
order. This model is erased; runtime traversal remains lazy. Checked hash/index
bijections connect it to the emitted hashes. The real `Iterator::next`,
`size_hint` and `ExactSizeIterator::len` satisfy both their concrete set/count
contracts and vstd's standard iterator model. A generic public consumer uses
ordinary `collect::<Vec<_>>()` to prove complete, duplicate-free, ancestor-first
results. Direct-step consumers check shrinking exact counts and fused exhaustion.
No adapter or application axiom is introduced.

`Commits` stores its cursor in an `Option`. `next` takes ownership before advancing
and restores it on normal return. If advancement unwinds, the iterator retains a
valid exhausted state; no caller can observe an invalid cursor through a surviving
borrow. This changes private representation only and adds no per-record allocation.
All public signatures and the interval/frontier representation are retained.

A nonempty public proof fixture checks actual hash emission despite denied
participation. Runtime regressions cover growth after capture, dropped capture
handles while the iterator lives, canonical order, exact hints/counts, both wrong-
history guards with equal contents, and repeated exhaustion. Twenty-eight new
mutations target identity guards, clock forwarding, hash projection, cursor
restoration, count/hint/model/invariant weakening and contradictory public consumers.
The local targeted run checks these after the full baseline; the complete 344-case
sweep is configured, not claimed rerun locally.

This verifies the public causal difference API, not policy-filtered materialization.
The separate `History::commits_since` wrapper is now checked in the local-query
milestone above and uses the same verified iterator. Owned traversal extension is now checked
below, followed by materialization payload/visibility mapping and batching.
Public completion is now checked above; matching application-state/checkpoint persistence remains open.

## Owned traversal across History growth

The production `Changes` path now retains
[`OwnedTraversal`](../stockfoil/src/verified/owned_traversal.rs). Its constructor
checks the actual clock-difference traversal under the existing clock-input
premises. Its type invariant retains an erased origin and persistent lineage
receipt. `next`, `remaining` and the identity guard require no caller validity
premises. This is an internal component, not yet the full public materialization API.

The actual Topology constructor, append and metadata paths maintain a stronger
`ReadState`: old parent edges and every old chain row remain prefixes of the
current graph/tables. Receipt compatibility supplies these facts after any number
of appends. [`traversal_extension.rs`](../stockfoil/src/verified/traversal_extension.rs)
proves unchanged captured positions, selected hashes, frontiers and eligibility;
existing ready keys and dependency waiters remain sound, with the same count.
These facts are proved from production storage updates, not assumed growth axioms.

Every successful owned step emits the minimum eligible captured hash, removes
exactly that hash, and decreases the count by one. New deliveries cannot enter the
capture. `None` means true exhaustion and stays exhausted; wrong histories return
exactly `WrongHistory` without changing the cursor. The existing numeric history
field becomes an `Identity`; snapshots and receipts are erased. No runtime graph
copy, eager commit list, per-record allocation or new library contract is added.
Like `Commits`, taking the cursor before advancing preserves a valid exhausted
state if advancement unwinds. Successful steps restore the cursor with a current
receipt.

[`owned_traversal_examples.rs`](owned_traversal_examples.rs) checks arbitrary
resumption and actual insertion before resumption, with no validity premises. A
nonempty witness captures a root, appends a same-author descendant, independent
root and merge, and proves that only the captured root can emerge. A runtime
regression compares paused/resumed changes against an uninterrupted baseline,
including order, payload IDs, visibility and completion cursor. It grows the
history between batches and exercises wrong-history checks at zero budget and
after exhaustion. Those runtime metadata checks do not claim a metadata proof.

The full baseline is 500 checks (208 strict). All 201 workspace tests, 22 audit
regressions and the all-features workspace check pass. The 21 new targeted mutations
are rejected with proof diagnostics; all 344 configured mutation anchors match.
The complete 344-case sweep was not rerun locally. The production view delegation
is now pinned by the wiring audit as well.

The planner milestone below now discharges the clock-input premises in
`plan_changes_at`. The emitter milestone below proves payload/visibility mapping
and batch/completion behavior. Public Materialization planning/completion are now checked above; in-memory driving and Ledger/Document persistence remain open.
These components do not verify the public protocol, publication/admission,
compaction, sync minimality or attestation completion.

## Actual materialization planning

The actual `history::view::plan_changes_at` entry point now re-exports checked
[`change_plan.rs`](../stockfoil/src/verified/change_plan.rs). No caller provides
clock ordering, chain bounds, graph validity or permission-compilation premises.
The function accepts arbitrary typed view/cursor values and a captured delivery
frontier, resolves all four head sources, compiles before/after permissions, checks
coverage and constructs the owned traversal. For successful planning, its exact
hash set is:

```
(newly captured delivery - previous delivery)
  union (previous authorized visibility - target authorized visibility)
  union (target authorized visibility - previous authorized visibility)
```

All selected records lie below the supplied captured delivery heads. Known later
deliveries do not silently expand this capture. A required head outside that
capture produces `InvalidItem`; an unresolved delivery/view head produces
`UnknownCommit` naming an unknown input head. Permission frontiers retain the
existing policy: missing bounds and bounds outside the selected view grant nothing,
rather than being treated as required view heads. A denied new delivery still
belongs to the delivery difference; this is not a payload-confidentiality claim.

The proof derives constructor premises from actual causal summaries and compiled
permission clocks. Causal closure lifts head coverage to coverage of all selected
ancestors. Clock membership is frozen against the planning state for subsequent
record-mapping proofs. The wrapper saves the exact supplied delivery-head sequence
and a semantically identical target, including listed and unlisted permissions.

The actual exported `ViewPoint` and `MaterializationCursor` types now live in
[`view.rs`](../stockfoil/src/verified/view.rs). Their constructors/defaults,
getters and clones are checked, as are `WritePermission`/`WritePermissions` clones
and `Changes::remaining`. Clone preserves bounded-frontier sequences and the
listed/unlisted distinction; a checked lemma proves copied views have the same
authorized set. Existing vstd Vec/HashMap clone specifications suffice. All public
signatures, equality behavior and serialized representations remain unchanged;
serialization/deserialization implementations are not verified by this milestone.

[`change_plan_examples.rs`](change_plan_examples.rs) exercises arbitrary planning,
public copying, a denied new delivery, a nonempty permission retraction, an empty
unchanged plan, unknown heads and insufficient coverage. Runtime tests exercise
each of the four unknown-head sources, each coverage guard, captured-frontier
stability, and unknown/out-of-view permission bounds through actual emission.

Validation: 209 strict checks and 530 full checks, 202 workspace tests, 23 audit
regressions and all-features compilation pass. All 28 new targeted mutations are
rejected with proof diagnostics; all 372 mutation anchors match. The complete
372-case sweep was not rerun locally. There remain 37 enumerated library contracts
and no application assumptions. The wiring audit guards both actual planner
forwarding and public view/cursor exports.

The emitter milestone below completes actual payload/visibility mapping, batch
limits and completion cursors. Subsequent milestones above compose the public
plan/complete/step/update protocol through Ledger/Document.

## Actual materialization emission and batching

The production `emit_next_changes` body and actual `Change`/`Batch` types now live
in [`change_plan.rs`](../stockfoil/src/verified/change_plan.rs). Both its single-record
step and batch entry point accept arbitrary typed values without caller validity
premises. A maintained plan invariant binds the numeric History identity and
lineage receipt to frozen delivery/before/after membership sets and their actual
clocks. Current History compatibility proves those clocks still answer the captured
questions after growth; later deliveries cannot become selected or visible.

Each emitted record names the retained node's hash and payload ID. Its
`newly_delivered` flag means absence from the previous delivery set, and its
Before/After/Both/Neither classification exactly represents captured permission
membership. Payload correspondence is to the stored node at emission; this
receipt does not independently snapshot payload bytes. Denied new deliveries
still produce metadata with no target participation. There is no payload-secrecy
claim.

Each successful batch appends exactly `min(max, remaining)` records, preserving
any preexisting output prefix. It removes precisely those hashes, maintains exact
counts, emits no duplicates, and chooses the minimum eligible hash at every step,
so selected ancestors precede descendants. Zero budget preserves traversal and
output. `More` means records remain; `Complete` means true exhaustion and returns
the exact saved delivery-head sequence and semantically identical target view.
Repeated completion appends nothing. Wrong History rejects before mutation even
at zero budget and after exhaustion, leaving both traversal and output unchanged.

The plan retains an additional numeric identity and optional cursor; its logical
sets, snapshot and receipt are erased. Taking/restoring the cursor maintains a
valid exhausted state on unwind. There is no new per-record allocation or runtime
graph copy. The normal-return contract does not promise transactional recovery
from a panic during batch emission or publication.

[`emission_examples.rs`](emission_examples.rs) checks arbitrary batching, zero
budget, complete planning-to-drain composition, repeated completion and emission
after insertion. The concrete planning fixture now emits its denied new delivery
and permission retraction after actual History growth, proving their payload IDs
and flags. Runtime regression coverage includes existing output, arbitrary typed
persisted cursors exercising all four visibility cases, split/unlimited batches,
later delivery, completion metadata and equal-content wrong histories.

Validation is 209 strict/542 full checks, 203 workspace tests, 24 audit regressions
and all-features compilation. All 31 new targeted semantic mutations are rejected
with proof diagnostics; all 403 configured anchors match. The complete 403-case
sweep was not rerun locally. The mutations exercise identity/receipt/frozen-clock
bindings, cursor restoration, wrong payload/hash/flags, output preservation,
limits, progress, exhaustion and completion metadata, plus contradictory generic
and nonempty consumers. A redundant cursor-lineage conjunct was removed after
its weakening still verified; that experiment is not counted as a rejection.
The audit now guards the actual emitter delegation. The trusted boundary remains
37 library declarations, with no application assumptions.

At that milestone, the public Materialization lifecycle still needed composition.
Public `plan`, `complete`, `step` and `update` are now checked as described above;
local edits and concrete application/durability proofs remain open.

## Immutable public batches and pending-work handshake

The actual exported `Batch` and `Event` representations now live in
[`batch_protocol.rs`](../stockfoil/src/verified/batch_protocol.rs). Public
`Batch::events`, `is_rebuild`, `len`, `is_empty` and clone are checked without
caller validity premises. Clone preserves every event's kind, hash, author,
ordered parents, payload ID and activity flag. The rebuild flag means that the
sequence contains a Reset. A logical event projection ignores vector allocation
details while retaining all application-visible fields.

The same module owns the production pending slot used by `Materialization`.
Installing work requires no caller validity premises: an occupied slot is rejected
without mutation; success saves the exact pending value, advances the ticket and
returns the exact supplied events. An erased issuance map and persistent batch
receipt bind those events to their planner/ticket. Checking a token returns the
matching pending value and proves event correspondence. Taking it consumes that
value and empties the slot. Stale or foreign tokens leave the slot unchanged.
Clones share the same ticket; after consumption none can complete it again, and
a follow-up uses a strictly later ticket. Tickets never wrap: the maximum ticket
can complete, but subsequent issuance returns `Exhausted` without altering state.

Planner construction now uses the existing checked, non-reusing numeric identity
allocator. The former `fetch_add` could wrap after exhausting planner IDs; the
shared allocator instead panics at identity-space exhaustion. Equal numeric IDs
prove equal issuance lineages using the existing persistent registry. A fresh
planner's empty issuance history proves it cannot adopt an already issued token.
The identity and ticket are still numeric runtime fields; issuance histories,
receipts and event projections are erased. There is no runtime log of old batches.

[`protocol_examples.rs`](protocol_examples.rs) checks arbitrary consumption,
occupied-slot rejection, clone invalidation, follow-up tickets, foreign planners,
public observations and a nonempty round trip containing all five event kinds.
A checked near-exhaustion witness issues and consumes the maximum ticket, then
proves subsequent issuance fails unchanged. A runtime boundary regression covers
the same last-ticket transition; existing public Materialization tests exercise
foreign/stale clones, payload follow-ups and rejected application reports.

Validation: 209 strict/565 full checks, 204 workspace tests, 26 audit regressions,
all-features compilation, formatting and rustdoc link checks. All 33 new semantic
mutations and four existing allocator/identity mutations are rejected locally.
All 436 configured anchors match; the complete sweep was not rerun. The audit now
pins `materialize.rs` to catch bypassed delegation or replaced public batch types.
There are still 37 trusted library declarations and no application assumptions.

This milestone verified the actual token and pending-value handshake, before the outer public
`Materialization::plan`/`complete`/`step`/`update` bodies were checked above. The pending value is generic:
the handshake preserves it exactly without assuming that its proposed checkpoint
is correct. Event translation, rejection validation and cascading corrections,
reset/replay progress, application callbacks and checkpoint publication still need
composition proofs. `Batch::resolve` and generic Event payload mapping remain
outside the harness. No rollback guarantee is made for source advancement or
application effects when a surrounding operation fails.

## Durable checkpoint values and transitions

The actual public `Checkpoint`/`Follow` and internal `InProgress` types now live in
[`checkpoint.rs`](../stockfoil/src/verified/checkpoint.rs). Public constructors,
defaults, clones, `Checkpoint::follow`, `heads` and `is_partial` are checked.
Cloning preserves the follow rule (including ordered pinned heads), completed
cursor, rejection map, captured target/delivery sequence and processed count.
`heads` reports the completed/base cursor's heads even when a partial target is
present. Arbitrary typed or persisted metadata needs no caller validity premise
for these operations; semantic graph validation is a separate obligation.

Materialization now calls three checked production helpers:

- `prepare_next` builds a proposed checkpoint without changing the currently
  published one. A supplied completed cursor becomes the result's cursor and
  clears partial progress. Otherwise the supplied base cursor remains, and partial
  progress records the exact target, captured delivery sequence and processed count.
  Both branches preserve the follow rule and all permanent rejections.
- `restart` empties the cursor and resets the processed count to zero while keeping
  the supplied captured target/frontier and the existing follow/rejection state.
- `reject` changes only the named hash's rejection. Cascade strength is the OR of
  the old and new flags, so it cannot weaken; the return flag is true exactly when
  the hash was previously absent. Unrelated rejections and all other metadata remain.

[`checkpoint_examples.rs`](checkpoint_examples.rs) composes actual planning,
emission and staging with arbitrary inputs: partial results retain the base and
capture metadata; exhausted results contain the target cursor. Another consumer
passes a real proposed checkpoint through the checked token handshake, proves it
remains pending until acknowledgment, preserves a changed follow rule and rejects
replayed clones. Concrete nonempty examples distinguish base heads from target
heads and check rejection preservation across completion and reset. These are
composition witnesses, not proofs of the outer Materialization methods.

A public runtime regression changes the follow rule during a payload follow-up,
checks that rejection state is not published early, repeats a rejection with a
weaker cascade flag, and round-trips the final checkpoint through the unchanged
codec. Existing tests cover partial resume after reordered graph reconstruction,
later deliveries, reset/replay and rejection persistence. Public paths, serialized
representations and checkpoint fields retain their existing meaning.

Validation: 209 strict/583 full checks; the workspace baseline and updated
materialization suite cover 205 passing tests, with two existing ignored doctests.
All 28 audit regressions, all-features compilation, formatting and rustdoc link
checks pass. All 30 new targeted semantic mutations are rejected; all 466 anchors
match. The complete mutation sweep was not rerun. The source manifest now guards
the checkpoint re-export and transition delegation. vstd's existing BTreeMap clone
and update contracts and the existing hash-order model suffice: there are still
37 trusted library declarations and no application assumptions.

The source milestone below establishes skipped counts, record progress and
completion metadata for the captured source. Event
translation, rejection-report validation, cascading corrections, follow-up/reset
control flow and publication through Ledger/Document remain open. Checkpoint
codec bodies and the `rejected()` iterator are outside this milestone; no durable
I/O atomicity or application-effect correctness is claimed.

## Captured source resume, progress and completion

The actual `Source` implementation now lives in
[`materialization_source.rs`](../stockfoil/src/verified/materialization_source.rs).
Its maintained invariant binds the processed count, remaining captured selection
and erased total, with `processed + remaining == total <= usize::MAX`. Any stored
completed cursor matches the captured delivery sequence and target, and completion
requires exhaustion. Source construction and methods need no caller validity
premises. The outer Materialization delegates resumption, advancement, checkpoint
preparation and reset-checkpoint construction to these checked bodies.

Construction calls the real planner, rejects an impossible saved count as
`Malformed`, then discards exactly that many records in chunks of at most 1,024.
The skipped records form a canonical prefix: each was the minimum eligible hash.
A checked concatenation lemma joins chunk schedules, and a prefix-uniqueness lemma
proves that changing chunk boundaries cannot change the skipped selection. An
arbitrary executable consumer compares actual resumed and uninterrupted sources
and proves exact remaining-set and processed-count equality for the same History
snapshot. It does not assume that an arbitrary persisted count is valid.

Advancement first checks History identity, leaving the entire source unchanged on
a mismatch. A pending Reset reserves one slot when the budget is positive, and is
returned once. Exactly the available record budget is consumed up to exhaustion;
the processed count advances by precisely the emitted length without overflow.
Records retain the emitter's payload/visibility, uniqueness and canonical-order
guarantees. Total, target, delivery frontier and captured permission sets remain
fixed even when History grows. Successful advancement marks completion exactly
when no selected records remain. A fully skipped constructor becomes complete on
its first advancement, matching the previous runtime behavior.

Checkpoint preparation now uses the source's actual base, saved target/frontier,
processed count and completion cursor. A partial checkpoint contains that exact
progress; a completed checkpoint has consumed the full captured total and names
the saved target/frontier. Reset checkpoints retain the same capture and permanent
rejections while resetting progress. No independently supplied count or completion
cursor is needed at these production call sites.

The source takes its state into a local before mutable advancement and restores
it on normal return. If that advancement unwinds, the surviving source rejects
advance/checkpoint/reset requests with `Poisoned`. This is not a transactional
recovery proof for event translation, application effects or the outer lifecycle.
The optional state is runtime data; the total and skipped-prefix proofs are erased.
Existing captures are reused for target/frontier storage rather than retaining
duplicate copies. No eager list of selected hashes is introduced. Raw History
traversal forwarding remains available to tests and the fuzzing feature.

[`source_examples.rs`](source_examples.rs) checks arbitrary advancement/checkpoints,
same-snapshot replay equivalence, growth, reset budgeting, malformed progress and
a nonempty fully resumed capture that stays exhausted after insertion. Runtime
regressions cross the 1,024-record replay boundary, insert later history between
batches, verify exact suffix payloads and checkpoint counts, test exhausted
identity guards, and exercise the poisoned representation. Existing public
tests still cover persisted resume, reordered arrivals and reset/replay.

Validation: 209 strict/601 full checks, 207 workspace tests (two existing ignored
doctests), 29 audit regressions, default/all-features compilation, formatting and
rustdoc link checks. All 34 new semantic mutations reject; all 500 anchors match.
The complete mutation sweep was not rerun. There remain 37 library declarations
and no application assumptions. The production source delegation is pinned by
the wiring audit.

Remaining: verify the outer choice of source inputs from Ledger/checkpoint,
application-report validation, correction
rounds and matching publication through Materialization/Ledger/Document. Replay
equivalence across reconstructed graphs or newly compiled captures after growth
is not established by the same-snapshot consumer; continuation of an already
captured source across growth is checked separately.

## Captured records to application events

The production loop in `Materialization::plan` now delegates to
[`materialization_events.rs`](../stockfoil/src/verified/materialization_events.rs).
This is the code executed by Cargo, including metadata reads, direct/cascade
rejection checks, event construction and the summaries passed to pending work.

- Metadata comes from actual topology storage. The proof establishes correct
  author selection and the complete stored parent sequence, retaining its order
  and repeated parents. Parent-index and arithmetic bounds follow from the
  maintained topology invariant; they are not new assumptions.
- A hash is rejected exactly when the checkpoint names it directly, or a saved
  cascade root is its strict ancestor. Unknown roots cannot acquire descendants
  through the query; a directly named hash remains rejected even when unknown.
  This helper is also used by the outer correction and local-edit paths, whose
  surrounding control flow remains unverified.
- Each new-delivery record produces Deliver with its original hash and payload
  ID and the actual metadata. It is active exactly when after-visible and not
  rejected. Inactive deliveries remain available for structural metadata.
  Old records produce Activate for After or Retract for Before only when not
  rejected; Both/Neither produce no transition. Reset, when requested, comes first.
  The contract specifies the entire ordered sequence, not only membership.
- A checked summary pass computes exact event counts, the delivered-hash set and
  the ordered list of active **new deliveries** used for rejection correction.
  It neither includes Activate-only hashes in that list nor counts suppressed
  source records as events. Source progress still counts every consumed record.

[`event_examples.rs`](event_examples.rs) composes actual Source advancement,
translation, checkpoint staging and Protocol issuance for arbitrary typed inputs,
without assuming valid records or correct authorization answers. It proves that
any active Deliver or Activate in the issued Batch is inside the source's frozen
authorized set and passes the rejection query. A nonempty checked witness exercises
all four visibility labels, new/old records, direct rejection and summary counts.
The translator's internal known-hash premise is discharged by the real emitter;
this consumer is a composition witness, not a verified public Materialization method.

Runtime regressions use a merge with a cascaded rejection, an unaffected branch,
and repeated ordered parents; another changes pinned views with suppressed
retractions/activations and checks that all source records still count toward
completion. Translation remains incremental over the current batch. Summary
construction replaces two event-filter passes with one; proof sequences and
sets erase from runtime code. Normal-return guarantees do not establish recovery
from a panic during event allocation or application effects.

The full-suite run also required explicit quantifier triggers and prefix-to-final
composition steps in the existing clock collector proof (`traversal_init.rs`).
Those changes affect proofs only; the collector's runtime behavior is unchanged.

Validation: 209 strict/619 full checks, 209 workspace tests, 31 audit regressions,
default/all-features checks, formatting and rustdoc link checks. All 33 new semantic
mutations and 10 existing collector mutations reject with proof diagnostics; all
533 configured anchors match. The complete mutation sweep was not rerun. The trusted boundary remains 37 contracts.
The feedback milestone below closes report validation and rejection recording.
Outer source-input selection, correction rounds and final state/checkpoint
publication remain lifecycle obligations.

## Application feedback admission and durable rejection recording

[`materialization_feedback.rs`](../stockfoil/src/verified/materialization_feedback.rs)
now owns the actual public Applied/Rejection values and internal Pending value.
The public names, fields and method signatures are preserved. There is no assumed validity
invariant on these values: callers may supply arbitrary reports and typed pending
values. Public Applied default/want and the Pending constructor have checked
bodies; the generic reason-conversion builder and derived clones remain outside
the proof boundary.

`Materialization::complete` now calls the checked `accept_report` body. It checks
the real immutable batch receipt, then validates every rejection, then validates
wanted hashes, before calling the only consuming Protocol operation. Rejection
hashes must be distinct within the report and members of the pending delivery set.
Wanted hashes are deduplicated into the existing BTreeSet and resolved against
actual History storage. All returned errors preserve the entire Protocol value,
including its pending data, ticket and receipts. A matching valid report returns
the exact pending value and consumes that token once. Success also preserves every
rejection's hash, reason and cascade flag and the exact wanted set. Invalid tokens
win over malformed reports; malformed rejections win over unknown wanted hashes.
UnknownCommit always carries an actual requested unknown hash.

The delivery set belongs to the original source batch and survives immediate
follow-up rounds. A report on a payload-only follow-up can therefore reject or
upgrade a rejection of an original delivery. Wanted hashes need only exist in the
supplied History; this validation does not require membership in the current
batch or authorized view. These are existing semantics, now specified explicitly.
They are not a payload-confidentiality guarantee. The outer History-identity and
checkpoint validation still precedes this helper and remains outside the proof.

The checked rejection-recording loop computes the exact map union of prior and
reported hashes. A cascade is true precisely when already true or reported true;
repeats cannot weaken it. The returned count is the number of distinct report
hashes absent from the original rejection map, including when the helper receives
repeated hashes directly. Follow, cursor and partial progress are unchanged. The
outer addition to cumulative counters and subsequent correction/publication
control flow are now checked in the complete/step/update milestones above. Recording guarantees concern normal return, not
transactional recovery from a panic after consuming a token.

[`feedback_examples.rs`](feedback_examples.rs) proves acceptance exactly when a
token matches and both report checks hold, unchanged state on every returned
error, retry after malformed feedback, and one-time consumption. A consumer
composes actual Source advancement, event translation/summary, Pending construction,
batch issuance, report acceptance and rejection recording. Thus its permitted
rejection set comes from actual emitted deliveries rather than a supplied model.
A nonempty witness checks duplicate wanted normalization and exact repeated-cascade
recording. The public Materialization methods themselves are still outside the
harness; these checked callees and consumers do not yet establish the whole API.

Runtime tests cover invalid reports retaining a usable token, error precedence,
known requests outside the current source batch, ordered duplicate normalization,
foreign/stale tokens, cascade upgrades during follow-up rounds, deferred checkpoint
publication and exact new-rejection counts. Validation: 209 strict/636 full checks,
212 workspace tests (two existing ignored doctests), 33 audit regressions,
default/all-features compilation, formatting and rustdoc links. All 34 new semantic
mutations reject; all 567 configured anchors match. The complete mutation sweep
was not rerun. No trusted contracts were added; the boundary remains 37.

## Lazy traversal: per-chain interval cursors

Production `Traversal` now calls the checked
[`chain_intervals.rs`](../stockfoil/src/verified/chain_intervals.rs) for its actual
per-chain state. It retains the existing `VecDeque<Range<usize>>` representation;
no selected-commit list or whole-graph snapshot is introduced.

Construction sorts the input nonempty ranges and proves that merging overlapping,
nested, duplicate and adjacent intervals preserves exactly their union. The
checked type invariant makes the retained ranges nonempty, ordered and disjoint.
`remaining` equals the cardinality of that union and cannot overflow, including
at `usize::MAX`. `first` returns the least selected position; consuming `advance`
removes exactly that position, decreases the count by one and preserves the
invariant. Consuming the owned cursor avoids exposing a temporarily invalid
invariant-bearing value across mutable standard-library calls.

`last_before` returns the greatest selected position below an exclusive endpoint,
including gaps. `last_node_before` composes the checked binary partition with
that result to return the greatest selected node at or before the dependency tip.
[`interval_cursor_examples.rs`](interval_cursor_examples.rs) proves complete,
strictly increasing, duplicate-free enumeration through these production methods,
checks a nonempty gap/overlap fixture, and proves framing under table extension.
This proof consumer is not the runtime multi-chain scheduler.

One new standard-library contract covers sorting ranges by their concrete start
field. VecDeque operations use pinned vstd contracts directly. Application merge,
count, progress and dependency logic remain checked. Fourteen new semantic
mutations are rejected with proof diagnostics. Runtime regressions compare all
3,375 triples of ranges over five positions against independent sets, checking
each dependency query before and after table extension; a second test covers
empty selections and maximum-width/end-point arithmetic. Existing long-chain
retention and canonical-order tests also run in the workspace suite.

This is a **conditional per-chain layer**; the complete constructor, transition
and public difference proofs compose it. Constructor inputs must be nonempty
ranges; node queries require a bounded selection and a sorted chain table. The complete constructor now derives those facts from clock-difference collection,
and the joint transition maintains the aggregate count and ready/blocked maps.
The public difference layer now also establishes captured-clock compatibility
and exact hash emission. See [PUBLIC_API_PROOF_BOUNDARY.md](PUBLIC_API_PROOF_BOUNDARY.md).

## Lazy traversal: ready and blocked queues

Production `Traversal` now owns the actual checked
[`schedule_queues.rs`](../stockfoil/src/verified/schedule_queues.rs) implementation.
It retains the same BTreeSet of ready hash/index pairs and HashMap of dependency
waiter lists. Checked operations insert exactly the requested ready entry, append
one waiter without replacing existing waiters, remove the minimum ready entry,
and detach exactly one dependency's whole waiter list before rescheduling it.
They preserve uniqueness across ready entries and blocked rows, disjointness of
the two queues and strictly earlier dependency IDs. Detached waiters are no
longer queued and are safe to schedule again under the freshness contract.

These are explicit pre/postconditions on normal return: callers must establish
queue validity, freshness when scheduling and an earlier dependency when blocking.
They are not a type invariant for the whole production scheduler. In particular,
`pop_ready` returning None means only that the ready set is empty; proving that
this means traversal completion still needs the pending-chain invariant.

[`scheduler_refinement.rs`](scheduler_refinement.rs) proves conditional progress:
if the queues partition all pending frontiers and every dependency is covered by
a pending frontier at or before it, the minimum-index frontier cannot be blocked.
Combining that result with exact causal readiness and the actual queue pop proves
minimum-ready-hash emission. These premises are explicit obligations of the
production scheduler, not application assumptions. Concrete consumers exercise
opposite hash/index orders, equal-hash ties, shared dependencies, wakeup,
reblocking and repeated removal without recreating waiters.

Three new library declarations cover tuple comparison laws, NodeIdx key laws and
BTreeSet::pop_first's ordinary minimum/removal behavior. The NodeIdx source joins
the pinned key dependencies. No contract assumes causal readiness, queue validity,
frontier coverage or scheduler progress. Runtime regressions compare 4,000 mixed
queue operations with independent maps and check every hash byte and distant
index bits. Fourteen new mutations target dropped/corrupted records, lost waiter
lists, double pop, false exhaustion and weakened progress/readiness premises.

The checked scheduling decision below now derives actual ancestry dependencies.
Traversal construction and `next` must still establish and maintain its premises,
including the pending-chain/queue partition and aggregate count. No additional
public API is claimed verified by the queue or scheduling layers.

## Production dependency scan and scheduling

Traversal delegates its actual scheduling decision to
[`pending.rs`](../stockfoil/src/verified/pending.rs). The checked scan uses the
real Topology's ancestry clock and the same lazy `BTreeMap::range(..idx)` iterator.
It computes ancestry only when an earlier frontier exists. Each pending entry
must name the first selected position on its chain; its intervals must be bounded
and canonical, and no two entries may represent the same chain.

From those explicit premises, `dependency` returns either a selected strict
ancestor covered by a pending frontier, or None with a proof that **no selected
strict ancestor exists anywhere in the pending intervals**. The frontier reduction
uses the stored graph's chain ancestry invariant, rather than assuming readiness.
The checked PendingChain methods derive table ordering and node labels from
Topology; `last_before` returns the greatest selected node at or before the clock
tip, including selections with gaps.

The actual `schedule` call preserves queue validity and adds exactly the candidate.
It inserts the stored hash precisely when the candidate is causally eligible;
otherwise it appends the candidate to a selected, strictly earlier ancestor's
waiter row, preserving other queues and entries. Queue validity and candidate
freshness remain explicit caller premises, not a whole-Traversal type invariant.

Three additional library declarations model only the opaque range iterator,
its exact restricted-map contents and ordered borrowed `next` results. No
Stockfoil selection or graph property enters these contracts. Runtime regressions
check boundary keys against an independent numeric filter, and exercise real
Topology scheduling with holes, an unrelated root, a dependency beyond its
frontier, and a selected descendant beyond the candidate's clock. Twelve new
mutations reject skipped/reversed ancestry tests, wrong dependencies/hashes,
omitted queue updates, weakened pending invariants and contradictory assertions
on nonempty paths. The whole 344-case sweep is configured but was not rerun locally.

The checked advancement and wakeup steps below preserve their local invariants.
The joint initialization/step and complete constructor proofs compose them, and
the public difference layer derives their input premises from captures. Owned
materialization traversal stability under history extension remains open.

## Production advancement and waiter wakeup

`Traversal::next` now delegates its actual map update and waiter loop to checked
`pending::advance` and `pending::wake`. Advancing removes exactly the emitted
record from the finite selected set and decreases its cardinality by one. It
preserves canonical, bounded intervals, unique chain ownership and exact frontier
keys; a surviving chain receives a strictly later frontier, without overwriting
another chain. Exhausted chains disappear. Every other map entry is preserved.

Wakeup detaches the entire waiter row before rescheduling. Each waiter is handled
once; every queued node remains queued, and unrelated ready entries and dependency
memberships are preserved. Released waiters either become causally ready with the
stored hash or block on a still-selected ancestor. Once the emitted node has left
the selection, its waiter row cannot reappear. These proofs use explicit valid-map,
valid-queue and waiter-membership premises; they do not assume wakeup makes every
waiter ready.

The finite selected set is derived from the real topology's indexed nodes and
pending intervals, independently of the executable `remaining` counter. Its exact
one-record decrease is now used by the joint step proof below to preserve equality
between the aggregate counter and that set's size. The initial interval collector establishes this equality, and public iterator
composition uses it for exact lengths. Owned history-extension stability remains open.

One additional library declaration supplies the pinned NodeIdx's ordinary derived
comparison laws so map insert/remove can use vstd's specifications directly. No
application contract is trusted. A nonempty proof fixture constructs Topology and
a pending map, schedules/pops its frontier, retires its chain and reaches exhaustion.
A runtime regression covers two shared waiters, one becoming ready while the other
reblocks, repeated wakeup, unrelated ready hashes and final chain retirement.
Fourteen new mutations cover skipped/double advance, wrong frontier removal or
reinsertion, lost/repeated waiters and a contradiction after actual construction.
The targeted local run also rechecks the previous twelve scheduling mutations,
verifying the changed pending module and its construction fixture after the full
baseline pass. The complete 344-case sweep is configured, not claimed rerun locally.

## Joint traversal initialization and step

The checked traversal constructor delegates queue initialization to
[`traversal_step::initialize`](../stockfoil/src/verified/traversal_step.rs), and
`Traversal::next` delegates the complete transition to the checked `next` body.
The joint invariant requires valid pending intervals/frontier keys, exact
ready-or-blocked membership for every pending frontier, correct hashes and causal
eligibility for ready entries, selected ancestors for blocked dependencies, and
`remaining` equal to the finite selected-set cardinality.

Initialization establishes that invariant from a valid pending map and exact
count, collecting only its frontier keys. Every step preserves the invariant,
removes exactly its returned node and decrements the actual counter once without
underflow. The emitted node has the minimum bytewise hash among all currently
eligible selected records (with the ready-set index tie-break). A None result
means the selection and counter are empty, not merely that the ready queue is
empty. The proof derives frontier coverage and exact eligibility from production
state before using the previously conditional progress theorem.

Between advancement and wakeup, only waiters on the just-emitted dependency may
have a stale dependency. New-frontier scheduling preserves that temporary
condition; wakeup restores selected dependencies everywhere. This establishes
the preconditions of each actual map, queue and scheduling operation, rather than
asking callers to supply them separately for every step.

A generic checked consumer repeatedly calls the real step and proves termination,
exact complete enumeration, no duplicates and ancestor-before-descendant order
at a fixed topology. A concrete proof constructs a nonempty Topology and pending
map, initializes it, emits its record and checks repeated exhaustion. A runtime
regression covers shared dependencies, reblocking, hashes that disagree with
ancestry order, exact counter updates and repeated exhaustion on a nonempty
history. Nineteen new mutations target missed transitions, false exhaustion,
wrong output/counts, corrupted frontier collection and weakened joint invariants.
These checks verify the new step and consumer modules after the complete baseline.

The constructor below now establishes this state precondition from clock inputs.
The public iterator type now maintains that joint invariant; its construction
derives clock compatibility and its next method proves exact hash emission. No library contracts or
application assumptions were added in this step; the count remains 37.

## Complete lazy traversal constructor

`Traversal::new` now delegates its entire construction to
[`traversal_init::new`](../stockfoil/src/verified/traversal_init.rs). The harness
checks the actual production collector, interval normalization, pending-map
construction, count summation and queue initialization. It requires ordered input
clocks, bounded `after` chain IDs and ordered stored chain rows; it does not assume
a valid pending map, exact count, dependency coverage or ready queue.

Collection produces only nonempty ranges bounded by their stored chain. Its
position-level contract is exactly the union of every requested `after - before`,
including overlaps, duplicate requests, empty/reversed differences and disjoint
ranges. A checked bridge relates it to the existing record-level `delta_member`
contract. `Intervals::new` preserves that selection while normalizing overlaps.
Each chain contributes its first selected node as a unique pending-map key; nodes
on different chains cannot collide or double-count. An injective position-to-node
mapping proves that interval lengths equal selected-record cardinality. The sum
is bounded by the number of stored nodes, ruling out integer overflow.

Construction keeps the interval representation. Existing rows use checked
`BTreeMap::get_mut`; normalization consumes rows using a temporary vector of chain
IDs and checked removals. It never expands all selected records. This replaces the
old unverified callback and consuming-map loop without adding library assumptions.

[`traversal_init_examples.rs`](traversal_init_examples.rs) composes this constructor
with the actual repeated step and proves exact union enumeration, uniqueness,
ancestor-first order, termination and true exhaustion at a fixed topology. A
nonempty construction witness requests the same difference twice plus an empty
self-difference and proves one emission and count one. The runtime regression
covers overlapping requests, a retained hole, two chains, canonical ready-hash
order and exact counts through exhaustion. Twenty-two new mutations target lost
ranges/pairs/chains, wrong endpoints/frontiers, duplicate or omitted counting,
weakened range bounds and a contradiction after construction. The targeted local
run rejects them with proof diagnostics; the full 344-case sweep is configured,
not claimed rerun locally.

This closes the internal constructor's pending-state/count gap. The public
difference proof now derives these clock facts from checked captures, maps indices
to exact hashes and maintains its iterator invariant. Its borrowed History stays
fixed during iteration.
Owned materialization traversals separately need selection/invariant stability
across history extension. The trusted library count remains 37.

## Topology validity and panic safety

Topology now maintains a checked type invariant rather than requiring membership
callers to supply `Topology::valid`. `Captured::new`, `check_history`, `contains`
and `participates` have no caller preconditions. An arbitrary-borrowed-value proof
witness checks both membership results and wrong-history rejection.

Production insertion reserves its vector/hash-map capacity first, with no logical
changes even on reservation failure. The new shared `append_plan.rs` then performs
only reserved updates. A checked non-unwinding publication step combines those
updates with delivered heads and the erased lineage. Metadata writes check the
index first and preserve the same invariant. The constructor establishes the
invariant before constructing the value. No graph copies, poison flag or runtime
proof resources are introduced.

The trusted boundary grows by 12 collection capacity/unwind contracts, and the
existing OnceLock-construction contract now records that it cannot unwind. These
assume standard-library behavior only; the reservation plan, graph/index/head
preservation and lineage publication are checked. Reservation failure now panics
before logical mutation, including allocation errors that ordinary vector growth
would abort on. Infallible B-tree allocation retains the standard abort-on-OOM
assumption. See [TRUSTED_STDLIB.md](TRUSTED_STDLIB.md).

Local validation covers 32 targeted mutations: 16 new reservation/publication/
invariant cases, four head updates, and twelve capture cases. They fail with
proof diagnostics. Exploratory omitted/stale lineage-publication mutations hit
the solver resource limit; they are inconclusive and are excluded from this
count and the expected-proof-diagnostic mutation suite. The verifier continues
to fail closed on resource limits. Runtime regressions cover failed preparation
and retry, both fresh/existing author rows, invalid metadata indices, and generic
payload destruction. The previous layer had 12 audit regression tests; the public layer adds two.

The exported wrappers, error mapping and stored-head normalization now compose
these invariants in checked code. The old ordinary append helpers remain
test/reference code; production uses the checked reserved path.

## Shared production code

The harness includes these exact source files; it does not verify substitute
implementations of their executable bodies:

| Source / operation | Proved contract | Production caller |
| --- | --- | --- |
| [`traversal_step.rs`](../stockfoil/src/verified/traversal_step.rs): queue initialization and complete `next` | Establishes/preserves the joint scheduler invariant from explicit initial-map/count premises; exact count, minimum eligible hash, no false exhaustion. | Actual lazy Traversal; the checked constructor below establishes its input premises; public `Commits` and internal `OwnedTraversal` now maintain this invariant across their supported History lifetimes |
| [`traversal_init.rs`](../stockfoil/src/verified/traversal_init.rs): complete lazy constructor | Exact union-of-clock-differences ranges, normalization, unique chain frontiers, exact non-overflowing count and initialized joint invariant. | Actual `Traversal::new`; public difference construction now discharges the clock premises |
| [`commits.rs`](../stockfoil/src/verified/commits.rs) and [`canonical_order.rs`](../stockfoil/src/verified/canonical_order.rs): public borrowed iterator | Maintained type invariant, exact hash-set removal, canonical causal order, standard Iterator/collect model, exact lengths/hints and fused exhaustion. | Public `Ancestry::difference` and `Commits::{next,size_hint,len}`; History/Ledger `commits_since` wrappers are checked in the local-query milestone |
| [`change_plan.rs`](../stockfoil/src/verified/change_plan.rs): actual captured planning and emission | Derives clock premises; exact delivery/authorized-visibility selection, validation and frozen record flags; stored payload mapping; exact budget-limited append, preserved output prefix, canonical causal order, exact remaining counts, saved completion cursor and unchanged state on wrong-history errors. | Actual `plan_changes_at` and `emit_next_changes`, used by Materialization; in-memory driver lifecycle and application persistence remain unchecked |
| [`view.rs`](../stockfoil/src/verified/view.rs) and permission clones | Public constructors/defaults, getters and clones preserve heads, listed rules, fallback and bounded-frontier sequences; copied views authorize the same set. | Actual exported ViewPoint/MaterializationCursor; codecs remain unchecked |
| [`batch_protocol.rs`](../stockfoil/src/verified/batch_protocol.rs): immutable batches and pending work | Exact public event access/clone, receipt-bound planner/ticket/events, exclusive pending installation, unchanged stale/foreign rejection, one-time consumption and non-wrapping tickets. | Actual exported `Batch` accessors/clone and production Materialization handshake; in-memory driver lifecycle and application persistence remain unchecked |
| [`checkpoint.rs`](../stockfoil/src/verified/checkpoint.rs): durable values and staging | Public construction/clone/follow/heads/partial status; exact partial/completed staging without early publication, reset metadata, rejection insertion with non-weakening cascade and exact newness. | Actual exported Checkpoint and production staging/restart/rejection helpers; public codecs and in-memory driving are checked in subsequent milestones, while application persistence remains a caller obligation |
| [`materialization_source.rs`](../stockfoil/src/verified/materialization_source.rs): actual captured source | Exact canonical replay prefix, saved-count validation, same-snapshot replay equivalence, maintained processed/remaining conservation, reset budgets, unchanged wrong-history errors, correct captured completion and checkpoint metadata. | Actual Materialization Source; in-memory driver control flow and application persistence remain unchecked |
| [`materialization_events.rs`](../stockfoil/src/verified/materialization_events.rs): actual event translation | Stored author/ordered parents, exact direct/cascade rejection, complete translated event sequence, exact counts/delivered set/active-delivery list; checked source-to-token consumer proves activations stay inside the captured authorized set. | Actual Materialization planning loop and rejection helper; application persistence remains unchecked |
| [`materialization_feedback.rs`](../stockfoil/src/verified/materialization_feedback.rs): actual report admission and recording | Pending construction; public Applied default/want; exact rejection/request validation before token consumption, unchanged Protocol on errors, preserved report details, one-time consumption, exact rejection-map union/cascade OR and new-hash count. | Actual Materialization plan/complete helpers; application persistence remains unchecked |
| [`materialization_corrections.rs`](../stockfoil/src/verified/materialization_corrections.rs): actual correction and restart helpers | Exact cascade selection, ordered retractions, remembered suppression, stored payload IDs and unique BTree requests; Reset-only decision and preserved captured restart; source/report/follow-up-token composition. | Actual Materialization complete helpers; application persistence remains unchecked |
| [`materialization_driver.rs`](../stockfoil/src/verified/materialization_driver.rs): actual public driver values and binding | Exact public construction/resume/accessors and follow framing; fresh-token isolation; Poisoned/WrongHistory/UnknownCommit precedence, complete binding-reference validation, preserved state on guard errors and one-time owner binding. | Actual exported Materialization and guard called by all lifecycle operations; application persistence remains unchecked |
| [`materialization_start.rs`](../stockfoil/src/verified/materialization_start.rs): source selection and installation | Exact follow targets; visibility difference against prior delivery; preserved partial captures; reset/empty-base decision; validated source installation with unchanged previous state on returned errors. | Actual startup and Ledger view helpers; application persistence remains unchecked |
| [`commit_metadata.rs`](../stockfoil/src/verified/commit_metadata.rs): preparation metadata | Explicit/default parent selection, first missing parent, exact author and parent preservation through clone/accessors/finish, and current signing bytes. | Actual helpers used by History/Ledger begin-commit and CommitBuilder; public `Prepared::signing_bytes`. Hash calculation and complete builder/commit wrappers remain open. |
| [`commit_builder.rs`](../stockfoil/src/verified/commit_builder.rs), [`commit_encoding.rs`](../stockfoil/src/verified/commit_encoding.rs) | Exact bijou64 parent count, ordered parents, author and streamed payload digest; clone and default/explicit parent selection. | Actual public History/Ledger `begin_commit` and `CommitBuilder`; BLAKE3 behavior is explicitly trusted. |
| [`document_commit.rs`](../stockfoil/src/verified/document_commit.rs), [`materialization_change.rs`](../stockfoil/src/verified/materialization_change.rs) | Native conversion/callback effects, exact bytes/parents/hash, storage/publication, acknowledgement and errors; successful change leaves current/dirty driver. | Actual public `Document::commit` and `Materialization::change`; concrete application-state correspondence and durable persistence remain open. |
| [`materialization_edit.rs`](../stockfoil/src/verified/materialization_edit.rs): local edit phases | Exact preflight precedence, native callback state/bytes/errors, unchanged preflight application state, callback-error poisoning and post-publication acknowledgement/error handling. | Actual helpers called by public `change`; the complete wrapper now verifies as described above; concrete state correspondence remains open. |
| [`publication.rs`](../stockfoil/src/verified/publication.rs), [`document_publication.rs`](../stockfoil/src/verified/document_publication.rs): prepared-commit publication | Exact new metadata, epoch/draft updates, duplicate preservation, first-missing-parent and exhaustion errors, Ledger author guard, and first-write payload storage including retained bytes after publication failure. | Actual public `History::publish_commit`, `Ledger::commit`, epoch/author accessors and Document post-hashing storage helper. Preparation/hash construction and the complete wrappers now verify as described above. |
| [`materialization_local.rs`](../stockfoil/src/verified/materialization_local.rs), [`local_queries.rs`](../stockfoil/src/verified/local_queries.rs), [`draft.rs`](../stockfoil/src/verified/draft.rs): local acknowledgement and query dependencies | Exact metadata, normalized parent-head sets, unchanged policy, sole delivery and participation/rejection checks; current/dirty checkpoint publication, pin advancement and returned-error framing. Exact stored parent/author queries and causal commits-since iterator. | Actual public `mark_local`, History/Ledger `commits_since` and Ledger metadata/ancestry queries; concrete application-state correspondence remains open. Draft itself carries no publication assumption. |
| [`materialization_update.rs`](../stockfoil/src/verified/materialization_update.rs), [`progress.rs`](../stockfoil/src/verified/progress.rs): public repeated driving and reporting | Exact finite composition of public steps, cumulative counters and rebuild OR, initial/final heads, success/current framing, precise later-step failure and post-success report-overflow boundaries; checked ticket progress. Public Progress Default/Clone/changed. | Public `Materialization::update`; lifecycle reachability and concrete application/durability proofs remain open. Callback termination and panic recovery are not proved. |
| [`materialization_step.rs`](../stockfoil/src/verified/materialization_step.rs), [`materializer.rs`](../stockfoil/src/verified/materializer.rs), [`drive_error.rs`](../stockfoil/src/verified/drive_error.rs): public in-memory step | Preflight framing; exact plan/batch/application/completion/checkpoint sequence; captured activation bound; exact callback error values; saved-checkpoint restoration and poisoning after returned failures; exact Current/More result. Callback relations use actual method contracts. | Public `Materialization::step`; lifecycle reachability and concrete application/durability proofs remain open. The driver loop has a decreasing ticket measure; callback termination and panic recovery are not proved. |
| [`materialization_complete.rs`](../stockfoil/src/verified/materialization_complete.rs): public completion | Exact feedback merge/counts, correction rounds, no follow-up activations, captured restart, fresh/stale tokens, deferred checkpoint/count publication, and precise validation-versus-consumed-error boundaries. Counter overflow and absent reset source return errors. | Public `Materialization::complete`; application persistence and general lifecycle reachability remain open |
| [`materialization_plan.rs`](../stockfoil/src/verified/materialization_plan.rs) and [`ledger.rs`](../stockfoil/src/verified/ledger.rs): public planning | Actual Ledger accessors, exact current-state predicate, structural policy/view equality, full plan control flow, canonical source advancement, budgeted authorized events, pending metadata/token and published-state framing; precise error precedence/origins. | Public `Materialization::plan`/`is_current`; application persistence remains unchecked |
| [`clock_equality.rs`](../stockfoil/src/verified/clock_equality.rs): actual clock equality | Stored-length and exact in-order sequence comparison for arbitrary clocks; exact map equality for valid clocks regardless of shape. | Used by the checked visibility-reset decision; no new library or application assumption |
| [`pending.rs`](../stockfoil/src/verified/pending.rs): frontier queries, advancement, scheduling and wakeup | Exact one-record removal/cardinality decrease; valid frontier replacement; causal scheduling and complete waiter rescheduling with unrelated entries preserved, under explicit state premises. | Actual Traversal scheduler; initialization/advance/wakeup must maintain the premises |
| [`schedule_queues.rs`](../stockfoil/src/verified/schedule_queues.rs): ready insertion, block, pop and waiter removal | Exact updates and uniqueness/disjointness preservation under explicit validity/freshness/dependency premises; minimum ready key uses the enumerated library contract. | Actual lazy Traversal queues; causal readiness and pending partition remain caller obligations |
| [`chain_intervals.rs`](../stockfoil/src/verified/chain_intervals.rs): normalization, count, first/advance and last dependency | Exact union of nonempty input ranges, cardinality without overflow, one-position removal and maximal selected dependency under explicit table bounds/order. | Actual lazy Traversal per-chain storage; scheduler/caller premises remain open |
| [`history.rs`](../stockfoil/src/verified/history.rs): public capture, membership, heads, clone, construction and strict ancestry | Exact frozen causal/policy sets, normalized requested heads and public errors, without caller validity premises. | Actual exported History/Ancestry methods |
| [`topology.rs`](../stockfoil/src/verified/topology.rs): constructor, insertion, metadata updates, cached/hash-resolved queries, strict ancestry | Establishes and preserves private storage/index/cache/chain invariants, resolves actual hashes, derives sound chain reuse and capacity, and proves exact parent-edge membership. Rejected insertion preserves logical state. | `History::new`, `insert`, flag updates, `clock`, `clock_for_heads`, and the checked public `is_ancestor` delegation; prepared-commit publication is checked above; admission/evidence protocols remain outside the harness |
| [`append_plan.rs`](../stockfoil/src/verified/append_plan.rs): preparation and reserved append | Preparation preserves logical state on success or failure; success establishes every required capacity. Reserved append cannot unwind and preserves exact storage/index/chain meaning. | `Topology::insert` through `ancestry_cache::append_prepared` |
| [`heads.rs`](../stockfoil/src/verified/heads.rs): set update and shortcut guard | Maintains the actual BTreeSet by removing parents and adding the new hash. Proves exact maximal-vertex membership after append and preservation under metadata updates. Count-and-membership guard on distinct resolved indices implies the whole delivered frontier. | `Topology::insert`, `history_clock`; derived CommitHash ordering is one explicit library assumption |
| [`clock.rs`](../stockfoil/src/verified/clock.rs): `NodeIdx::new`, `index` | Preserve the exact `u32` position; the tuple field remains private. | History insertion and node indexing |
| `contains` | Inclusive membership in an optional coordinate; `None` differs from position zero. | `Clock::contains` and `include`'s early return |
| `cap` | Exact prefix intersection, including absent and zero bounds. | `Clock::mask` |
| `upper_bound` | Terminates and returns exactly the first index above a bound in a nondecreasing slice; arithmetic and indexing are safe. | Interval difference |
| `difference_range` | Valid half-open slice containing exactly the nodes in the after-prefix and outside the before-prefix. Reversed bounds give an empty slice. | `clock::difference`, hence fragment membership and materialization planning |
| [`difference.rs`](../stockfoil/src/verified/difference.rs): `difference`, `symmetric_difference` | Append exactly every selected chain-table node, preserving the existing output prefix and chain traversal order; prove indexing and termination. Sorted tables and clock-chain index bounds remain explicit premises. | Materialization candidates and fragment members |
| [`events.rs`](../stockfoil/src/verified/events.rs): `classify` | Both visibility bits are represented exactly by `Vis`. | Materialization record emission |
| `batch_end` | Advances by `min(max, remaining)` without overflow. Positive budgets make progress; zero budgets preserve position. | `emit_next_changes` |
| [`tree.rs`](../stockfoil/src/verified/tree.rs): `Clock::default`, `clone` | Empty clocks are ordered and have an empty coordinate map; cloning preserves the logical tree using `Arc::clone`. | Clock construction and snapshots |
| `Clock::node`, `height`, `len` | Constructor preserves ordering and the exact coordinate map, and computes exact structural height/size without overflow under explicit child and capacity premises. Accessors return cached metadata, which equals structural values when metadata is valid. | Rotations, updates and reconstruction |
| `ClockBuilder::default`, `include`, `merge`, `finish` | Empty construction and accumulation preserve the checked clock invariant; merges take pointwise maxima and finish returns exactly the accumulated map and size. | Cached ancestry walks and multi-head summaries |
| `Clock::mask` | Exact coordinate-wise intersection with the callback bounds, preserving the tree invariant and never increasing size. Denied keys disappear, zero remains a valid tip, and retained tips are capped. | Permission participation |
| `Clock::from_sorted`, `from_tips` | Sorted reconstruction preserves the exact entry sequence and establishes ordering, metadata and AVL balance. Tip-table construction maps precisely the contiguous chain IDs to their supplied tips. Both prove capacity, indexing safety and termination under an input-length bound. | Permission masks and delivered-history clocks |
| `Clock::union` | Exact union of keys and pointwise maximum tips; preserves the tree invariant and computes exact distinct-key size. The result must fit u32; overlap is counted only once. | ClockBuilder and ancestry summaries |
| `Clock::entries`, `Entries::next` | Actual Rust iterator returns precisely the remaining in-order sequence, terminates and stays exhausted. For ordered trees, the sequence contains every map entry exactly once in strictly increasing key order, with length equal to structural size. | Union, masking and interval differences |
| `Clock::set`, `include` | Terminating path-copying replacement and pointwise maximum update preserve ordering, metadata and balance, with exact size changes and height growth at most one. Only a new coordinate needs size capacity. | Clock builders and union |
| `Clock::balance` | Given balanced children whose heights differ by at most two, restores AVL balance while preserving ordering, the exact map and size, and valid metadata. Output height lies between the taller input child and one more than it. Explicit size/height capacity premises cover every intermediate constructor. | Path-copying insertion |
| [`chain_selection.rs`](../stockfoil/src/verified/chain_selection.rs): `direct`, `ancestral`, `register`, `append` | Direct search returns the smallest same-author current-parent chain; fallback returns the first covered tip in the supplied list. Registration and append preserve chain authors and exact storage updates. [`Author::same_as`](../stockfoil/src/verified/author.rs) checks all 32 bytes. | `Topology::assign_chain` and `insert` now derive author-list and ancestry-clock correctness from their maintained invariant |
| [`ancestry_cache.rs`](../stockfoil/src/verified/ancestry_cache.rs): append, read, initialize, multi-head query | Actual append creates and maintains each cell's ancestry/label predicate. The actual initializer proves its walk result satisfies it; reads derive correct current-graph summaries from the maintained binding. | `Topology::insert`, `clock`, `clock_for_heads` now discharge cache, chain and capacity premises |
| [`ancestry_walk.rs`](../stockfoil/src/verified/ancestry_walk.rs): `compute`, `combine` | The actual stack/visited walk terminates and constructs exact maxima over parent-edge ancestry; cached subgraphs and multi-head unions preserve that meaning. Repeated parents/heads, empty heads, and the single-head sharing fast path are covered. | `History::clock` and `clock_for_heads`; correct cache reads, linked storage and finite clock capacity remain explicit caller premises |
| [`indexes.rs`](../stockfoil/src/verified/indexes.rs): hash resolution, reservation and reserved author/hash updates | Exact hash-to-node correspondence, preserved parent order and repeats, unknown-hash rejection, complete ordered per-author chain lists, and preservation through actual storage append. | `append_plan`, hash resolution and chain fallback; topology establishes caller invariants |
| [`storage.rs`](../stockfoil/src/verified/storage.rs): `empty`, `Node::new`, `parent_slice`, reference `append` | Actual node storage starts valid; append preserves packed parent spans, parents-before-child order, existing nodes, exact new metadata and chain-table membership derived from the stored node labels. Parent slicing returns exactly the stored span. Capacity, resolved-parent validity and selected-chain bounds are explicit premises. | Constructor, node construction and parent reads; ordinary append is now test/reference-only |
| `chain_tables::append` | Actual writes to tips and rows preserve nonempty strictly ordered rows and exact last-node tips; extend precisely the selected row or allocate the next contiguous row. | `storage::append` derives labels and fresh indices; the owned Topology selects a sound chain |
| `candidates::normalize` | Exact set preservation and strict numeric ordering, conditional on the two enumerated stdlib contracts. History head-index resolution uses this same wrapper. | History::history_clock |
| `Clock::position`, `contains` | Terminating, exact lookup and inclusive membership in the actual tree, conditional on its BST ordering invariant. | History, permission bounds and interval differences |
| [`write_permissions.rs`](../stockfoil/src/verified/write_permissions.rs) | Actual default/new, insert/remove, fallback updates/access, and effective rule lookup preserve the author map and implement explicit-entry precedence. Default is denied. | Public WritePermissions methods; iteration and From/FromIterator conversions are outside the harness |
| [`identity.rs`](../stockfoil/src/verified/identity.rs) | Checked CAS allocation registers fresh numeric IDs against lineages. Persistent binding agreement proves equal runtime IDs have equal lineage IDs. Clone preserves the binding. Standard atomic/static/Option declarations are explicit. Executable proof witnesses cover a saturated allocator and a cloned identity. | Topology and Captured own the relocated ID fields; public History still needs the invariant composition proof |
| [`captured.rs`](../stockfoil/src/verified/captured.rs) | Actual capture and clone preserve frozen causal/authorized hash sets. A checked type invariant establishes handle validity. Current hash lookup and stored-chain queries derive compatibility from a tracked lineage receipt and return exactly the frozen sets, including false for newly delivered or unknown hashes. The runtime ID guard is checked before lookup; metadata is erased, and storage is the relocated u64 ID plus two shared clock pointers. | `Ancestry` owns this object and delegates membership; numeric identity now establishes logical lineage equality; whole-History validity and the public wrappers remain obligations |
| [`permissions.rs`](../stockfoil/src/verified/permissions.rs): `bounded_clock`, `participation_in`, `capture_clocks` | Actual hash filtering, per-author memoization and masking implement the independent policy. The combined entry point derives all clock premises from Topology, rejects unknown selected heads and proves exact causal/authorized membership for stored nodes. | `Ancestry::new` and materialization view planning; surrounding public handles and wrappers remain unverified |

`NodeIdx` stays a distinct history-local newtype; chain IDs remain `u32`.
The actual `Clock(Option<Arc<Entry>>)` representation and iterative lookup now
live in the shared `verified/tree.rs` module. `contents` gives its mathematical
coordinate map and `ordered` requires strict BST ordering throughout the tree.
Lookup follows a smaller subtree on each iteration; its termination proof does
not rely on cached height/length metadata. No tree copying or recursive runtime
lookup is introduced. The clone proof uses vstd's trusted `Arc::clone` contract;
a runtime pointer-equality regression also checks structural sharing.

**Whole-clock callers must establish validity and size capacity.** The owned
Topology discharges these obligations for its ancestry queries, and the shared
`capture_clocks` entry point derives the permission compiler's clock/owner premises.
Concrete numeric identity and guarded membership are checked. Whole-History
validity and the public wrappers still need proofs.
The actual `Clock::node` now lives in the shared verified module. Its children
must be ordered, have exact metadata, and have keys strictly below/above the new
root. Their combined size plus the root must fit `u32`, and their heights plus
one must fit `u8`. Under these premises, it preserves ordering and contents and
computes the exact structural size and height. These capacity conditions are
proof obligations. Topology insertion now supplies explicit node, parent and
chain-count checks; this does not prove capacity or failure behavior for all
other public operations.

`size` and `depth` are independent recursive specifications; `metadata` requires
cached values to equal them throughout the tree. Empty clocks establish this
predicate. The recursive `balanced` predicate independently requires child
heights to differ by at most one at every node. `node` characterizes when its
result is balanced; it also permits temporary unbalanced nodes used by rotations.

The production `balance` routine now lives in the shared module, with helpers
for the left-heavy and right-heavy cases. It verifies all four rotation paths
and the no-rotation path. Its inputs must be ordered, balanced, have valid
metadata, partition keys around the root, differ in height by at most two, and
meet the constructor's total-size and input-height capacity bounds. The proof
establishes each intermediate constructor's premises, safe height comparisons,
and the nonempty children needed by `unwrap`. Returned height is between the
maximum input child height and that height plus one. Unchanged subtrees still
use shallow `Arc::clone`; no whole-tree traversal or copying is added.

Insertion (`set`/`include`) now establishes the balancing premises along every
recursive path. It preserves ordering, metadata and balance, writes exactly the
requested coordinate (`set`) or its maximum with the old tip (`include`), and
increases size exactly when adding a key. Recursion terminates on a smaller
subtree; returned height never decreases and grows by at most one.

A structural counting lemma proves that any balanced tree with valid `u32`
size metadata has height below 65. Thus insertion needs no separate height
assumption. It still requires valid input and capacity for a new coordinate;
whole-clock construction and history callers must establish those premises.
The actual stack-based iterator is now a named `Entries` type in the shared
module. Its `Iterator::next` implementation verifies both its concrete remaining
sequence contract and vstd's standard iterator contract. Each descent terminates
on a smaller subtree, each yielded entry removes exactly the first remaining
item, and exhaustion is stable. A structural lemma connects that sequence to
the coordinate map: every map entry appears exactly once, in strictly increasing
key order, and sequence length equals structural size. No whole-tree staging
allocation is introduced; the traversal retains its stack of borrowed entries.
The production union now composes the verified iterator and inclusion operation.
Its loop accumulates exactly the maximum over the consumed entries and retains
the larger input's structure. The result contains exactly the union of input
keys, each tip is at least both applicable input tips, and every output tip is
one of those inputs. This characterizes the pointwise maximum without inventing
entries. A map-cardinality lemma connects ordered-tree size to distinct keys;
capacity is required only for the union of keys, so overlapping inputs are not
charged twice. The output has exactly that size and preserves the tree invariant.
Sorted reconstruction now establishes the full tree invariant directly. It
preserves the input sequence exactly, computes its exact size and midpoint-tree
height, and proves balance using a height-step lemma. Recursive calls consume
strictly smaller slices. Input length must fit `u32`; derived child height bounds
discharge constructor capacity without an independent height assumption.
`from_tips` constructs the same indexed entries as before, with a separate lemma
connecting each array index to its coordinate and excluding all other keys.
Masking now composes the actual iterator, shared cap operation and sorted
reconstruction. Its contract requires a callable, deterministic callback contract
for the relevant coordinates. It proves a bound result exists for every input
coordinate and returns exactly the keys whose bound is present, with each tip
capped at that bound. Size cannot increase. The implementation uses the existing
filtered-entry vector and visits each input once; no new runtime parameter or
whole-history scan is added by masking. The production permission compiler now
supplies the policy's correct bounds. It first walks selected clock entries to
build one bound clock per listed author plus one shared fallback clock, then uses an immutable mask callback. This adds
one traversal of the clock, preserves per-author memoization, and avoids any
whole-history scan. Unknown/out-of-selection frontier heads are removed before
ancestry. Frontier indices use the existing checked sort/dedup adapter.

The actual `ClockBuilder` now lives beside the verified tree and composes its
checked inclusion and union operations. It preserves the clock invariant and
returns precisely the accumulated map and size. The checked cached walk now
connects that map to the parent-edge graph and derives builder capacity from the
number of distinct stored chains. Its real `Vec` stack and `HashSet<u32>` visited
set are included in the proof. Each first visit reduces the finite unvisited set;
each repeat removes one pending occurrence. Cache shortcuts add a full ancestry
summary, and the multi-head path merges the head summaries without expanding a
whole-history list. All ghost sets disappear from the Cargo build.

The visited set uses primitive numeric keys, covered by existing vstd contracts;
the walk itself adds no project assumptions. The subsequent cache layer uses the
four generic OnceValue declarations documented in [TRUSTED_STDLIB.md](TRUSTED_STDLIB.md).
The production read and initialization closures now live in the checked module.
Actual append creates and preserves the metadata binding between each cell and
its ancestry at creation; proof snapshots are erased rather than stored as graph
copies. The initializer discharges the publication predicate using the actual
walk, and reads transport older snapshots to the current graph. Correct cache
contents are derived through this application proof, not assumed by the library.
History's complete state, chain-choice and capacity invariants still need to be
established at public entry points.

The callback specification uses a chosen result satisfying its contract;
existence for all visited coordinates is established by the actual calls, and
uniqueness is an explicit premise. No callback outcome is assumed without a
call. Its executable interface remains `FnMut`, allowing the existing bound cache.

Whole-clock difference enumeration now composes actual tree iteration, lookup
and interval search. It appends exactly the selected nodes to the existing
output vector, in chain traversal order, and preserves the previous prefix.
A membership lemma states precisely which table nodes occur. Symmetric difference
appends both directions. The private callback interface is replaced by a direct
vector argument at materialization and fragment callers, so the proof covers the
actual output writes; no staging allocation or whole-history scan is added.

The table rows must be sorted and every clock chain must index an existing row.
Concrete history construction must still establish table completeness, correct
chain membership and uniqueness. Enumeration is not globally topological across
chains; the new production Traversal retains ranges and schedules ready hashes. The
old normalized assembly remains a reference set model. Complete table layout,
clock meaning and canonical lazy scheduling remain concrete history obligations. `batch_end` also
requires `next <= total`; captured traversal construction must establish this.

## Connected model theorems and refinement

These files contain verification-only models and their refinement connections.
Their hypotheses are explicit proof obligations for the concrete wrappers, not
facts established about all runtime states. `tree_refinement.rs` calls the actual
production mask and connects its result to the independent graph policy.

| File | Checked result | Remaining production boundary |
| --- | --- | --- |
| [`chain_selection_refinement.rs`](chain_selection_refinement.rs) | Actual selectors yield sound same-author ancestral reuse under their input premises. Stored table tips are maximal; fresh IDs are unused. Actual append preserves the graph's single-author chain ancestry invariant. Fork, merge and cross-author relay witnesses compose the production functions. | Topology now supplies the maintained author index, real ancestry clock, hash resolution and capacity; public ancestry wrappers now compose these results; admission/publication remain separate. |
| [`storage_refinement.rs`](storage_refinement.rs) | Packed parent links project to a parent-edge DAG. Append adds exactly the supplied parents and preserves every old reachability relation. A fork/merge witness executes the actual append with repeated parents. | Hash identity, same-author ancestral chain selection, cache values and admission/public API semantics remain separate. |
| [`chain_table_refinement.rs`](chain_table_refinement.rs) | Actual table append preserves complete/correct node membership from the empty layout. Valid table tips produce a delivered clock covering every recorded node. | Storage supplies actual labels and fresh indices; selection refinement supplies maximal-tip reuse. Topology now establishes hash resolution, the author index, ancestry summaries and capacity; admitted-head and public capture contracts remain. |
| [`candidate_refinement.rs`](candidate_refinement.rs) | Reference eager assembly establishes the old batch model’s pending contract under explicit complete-table and delivered-clock premises. Numeric projection preserves membership; the model then gives uniqueness and topological order. A nonempty harness calls that reference assembler; it does not execute Traversal. | Concrete table construction, ancestry and permission clocks, captured traversal fields and public wrappers. |
| [`tree_refinement.rs`](tree_refinement.rs) | Actual tree entries refine the coordinate model; actual lookup equals coordinate membership; actual mask output equals graph authorization under history-summary and compiled-bound premises. A nonempty witness executes the production mask with an inclusive frontier. | Topology and the new compiler now establish ancestry/permission meaning directly. Concrete capture and public wrappers remain; this is not a public History API proof. |
| [`index_refinement.rs`](index_refinement.rs) | Actual hash resolution returns stored positions; complete author lists discharge fallback search premises; combined storage/index append preserves the graph chain invariant. A nonempty witness exercises interleaved authors, repeated parents and unknown hashes. | Topology now establishes freshness, valid ancestry clocks, selected-chain soundness and capacity; admitted-head, capture and public wrappers remain. |
| [`graph.rs`](graph.rs) | Independent parent-edge reachability is ordered and transitive; append preserves old reachability; same-author maximal-tip reuse preserves chain invariants; valid summaries imply exact ancestry membership. | The owned Topology now connects these to hash lookup, chain assignment, actual summary construction and the strict-ancestor query. Public wrappers and whole-History validity remain. |
| [`walk.rs`](walk.rs) | Starting, expanding parents, revisiting and substituting a complete cache preserve the walk invariant; an empty pending set yields exact history. The production walk now establishes this invariant from its actual stack/visited state. | Selective cache initialization/publication and whole-History invariants remain separate. |
| [`ancestry_refinement.rs`](ancestry_refinement.rs) | Actual walk summaries plus the chain invariant give exact ancestry membership. Appending packed storage leaves old cached summaries valid. A nonempty witness executes cold/cached fork-and-merge walks, repeated parents and heads, empty/singleton multi-head paths and reuse after append. | The owned Topology now maintains the cache/chain invariant and capacity through production insertion and metadata updates. Captured public membership remains separate. |
| [`head_refinement.rs`](head_refinement.rs), [`head_examples.rs`](head_examples.rs) | Every finite DAG vertex reaches a maximal vertex; appending removes exactly the incoming parents from the frontier and adds the new node. Actual hash queries exercise empty/duplicate/full/subset heads, unknown hashes and metadata updates. | Delivered-head/query construction is checked. Public wrappers, whole-History validity and admitted-head selection remain separate. |
| [`topology_examples.rs`](topology_examples.rs) | Starts from the actual empty owned state, inserts interleaved authors and repeated parents, proves positive and negative ancestry on a fork, rejects duplicate/unknown parents without logical changes, and preserves queries across metadata updates. Numeric capacity witnesses exercise the last chain ID and parent-space exhaustion. | The constructor/mutator/query implementations are checked; the outer History admitted heads, capture and public wrappers remain separate. |
| [`cache_examples.rs`](cache_examples.rs) | Nonempty execution of actual cache-bearing append, initialization, old-head reuse after append and repeated-head accumulation. A generic-cell witness uses an unrelated numeric predicate and shows a false-predicate cell cannot return a value. | Library implementation correctness remains trusted; ancestry API composition is checked; other History protocols remain unfinished. |
| [`capture_refinement.rs`](capture_refinement.rs), [`lineage.rs`](lineage.rs), [`capture_examples.rs`](capture_examples.rs) | Append preserves old coordinates and excludes new hashes from old positions. An authoritative tracked map and persistent receipts derive capture compatibility from logical lineage equality. Actual construction, append, clone and query maintain/use that relationship, without caller proofs after each mutation. | Numeric history IDs now establish logical lineage equality; whole-History validity and the public wrapper remain obligations. No lineage storage exists at runtime. |
| [`permission_refinement.rs`](permission_refinement.rs), [`permission_examples.rs`](permission_examples.rs) | Actual permission variants and known frontier hashes project to the independent policy. A nonempty consumer executes real topology construction and permission capture, including unavailable descendants, duplicate/unknown frontiers, cross-author inclusive bounds and all permission cases. | Captured-clock stability is checked under compatible extension; public wrappers, whole-History validity and materialization emission remain. |
| [`authorization.rs`](authorization.rs) | Independent default-deny policy laws: missing/denied permissions, exact unbounded permission, inclusive bounds, author isolation, available-frontier equivalence/monotonicity, and fixed-view stability under append. | The compiler maps actual hashes and stored authors to the independent allowed predicate, supplying the effective explicit/fallback rule. The captured object now has checked stability under compatible extension; whole-History validity and public wrappers remain. |
| [`visibility.rs`](visibility.rs) | Coordinate masking equals the independent parent-edge permission policy under explicit bound-compilation contracts. Unavailable frontier heads grant nothing. | The actual permission compiler now proves these facts directly against the policy; public ancestry composition is checked; admission and materialization remain. |
| [`intervals.rs`](intervals.rs) | The shared executable interval search selects exactly the supplied chain nodes in one history and outside another, given valid summaries and correct AVL lookup results. Also states the inclusive-head-minus-boundaries fragment rule. | Complete chain tables, whole-clock enumeration, AVL lookup, fragment boundary discovery and placement checks. |
| [`materialization.rs`](materialization.rs) | Candidate sets equal new delivery union visibility change. A batch model uses the shared boundary and classifier, emits the selected pending slice with exact fields, and consecutive slices concatenate without gaps or overlap. Sorted candidates are unique and topological; an unchanged cursor yields no candidates. | Concrete history/permission premises, record/hash/payload mapping, history identity checks, captured heads and completed-cursor publication. |
| [`difference_examples.rs`](difference_examples.rs) | Executes complete enumeration with two interleaved chains, reversed bounds, zero versus absence, both symmetric directions and a preexisting output marker. | Topology maintains concrete table invariants; the public difference layer composes the lazy collector and iterator. |
| [`examples.rs`](examples.rs) | Nonempty witnesses for chain summaries, permission masking and batches; zero versus absence, reversed bounds, maximum indices and budgets. | Specification checks, not wrapper proofs. |
| [`authorization_tests.rs`](authorization_tests.rs) | Nonempty policy witnesses with multiple heads, cross-author bounds, denied intermediate authors, unavailable heads, and a frontier becoming known after capture. | Specification checks, not wrapper proofs. |
| [`tree_examples.rs`](tree_examples.rs) | Actual shared tree witnesses exercise both search directions, absent keys, zero/maximum IDs, clones and membership. A malformed tree fails the ordering predicate. | Also uses actual construction and balancing for all four rotations, the no-rotation case, and equal-height grandchildren, with distinct tips and retained snapshots. Update witnesses also check replacement, monotone inclusion, extreme keys and retained snapshots; iteration witnesses check key/tip order, completeness and repeated exhaustion. Union witnesses cover overlapping keys, maximum tips, idempotence, both size-selection branches, empty inputs and retained snapshots. Reconstruction witnesses cover sparse/extreme keys, even sizes, empty inputs and zero tips. Mask witnesses use actual callbacks for denied, unbounded, zero and expanding bounds. The production permission compiler is now checked; concrete public capture and wrappers remain unproved. |

[`coordinates.rs`](coordinates.rs) supplies a mathematical sequence view of AVL
entries. It is not an executable vector clock. `tree_refinement::entries` now
widens the actual tree's `u32` keys to model `usize` labels, and proves sortedness
and membership agree with actual lookup. `masked_coordinates` connects actual
mask postconditions to coordinate permission intersection. `mask_for_policy`
executes the production mask and composes that result with `mask_matches_policy`.
Stored history authors are still abstract integer labels; their mapping and the
summary/compiled-bound premises remain unproved at public entry points. `graph::summarizes` requires each coordinate to be a
real history member and to bound every history member on its chain. Exact
membership is derived from these facts and the chain ancestry invariant.

The graph model follows parent edges independently of clocks. Repeated edges
have set semantics. Bounded permission first intersects the frontier **heads**
with the selected history, then takes their ancestry. Taking all frontier
ancestry first and intersecting afterward would incorrectly grant ancestors of
unavailable heads; a concrete witness and mutation test check this distinction.

The materialization model describes the current captured candidate list, not a
whole-history production scan. `emit_batch` is an executable **model** whose
boolean inputs stand for already computed delivery and visibility membership.
Only `batch_end` and `classify` are shared with the production emitter. A positive
budget decreases the number of candidates remaining; the host must supply such
budgets to finish a nonempty traversal. Later deliveries require a new traversal.

## Validation and trust

The initial sixty-nine mutations cover an expanding cap, exclusive membership,
exclusive upper-bound search, dropping position zero for an absent clock,
failure to advance a positive batch, swapped visibility, omitted new invisible
commits, granting an out-of-history frontier, defaulting missing permissions to
unbounded, escaping the selected history, excluding the frontier itself, and
selecting permissions by the wrong author, taking either wrong AVL subtree,
omitting a matching tree root, dropping a tree during cloning, choosing the shorter
child height, omitting the root from the size, swapping constructor children,
using the wrong tip in either rotation direction, skipping either inner double
rotation, skipping the left-heavy balancing case, ignoring replacement tips,
dropping a new coordinate, skipping every inclusion update, descending the wrong
iterator subtree, revisiting the left subtree, dropping a yielded entry,
ignoring the smaller union input, replacing union tips with zero, and discarding
the larger initial tree, replacing reconstructed tips with zero, swapping
reconstructed children, shifting tip-table chain IDs, failing to cap expanding
mask bounds, dropping position zero, ignoring denied mask coordinates, shifting
chain IDs in the model projection, dropping projected permission bounds,
skipping selected difference nodes, emitting the wrong node, and omitting the
reverse symmetric-difference direction, ignoring builder coordinates, discarding
previously accumulated inputs during a merge, dropping the finished result,
clearing the normalized candidate set, dropping new deliveries or visibility
changes during candidate assembly, and shifting projected node indices. A
nonempty consumer must also reject an inserted `assert(false)`; this checks for
exposed inconsistency in that witness, not global consistency of the contracts.
Three table mutations change the current tip, clear existing members, or omit the
first node of a newly allocated chain. Six storage mutations corrupt parent spans,
insert a self-parent, overwrite a chain, clear previous nodes, or read the wrong
parent slice. Eight selection/identity mutations drop the final author byte,
ignore author identity or tip freshness, reverse direct selection order, bypass
ancestry coverage, skip fallback candidates, or omit/corrupt author registration.
The index layer adds eight mutations for wrong/missing hash publication, altered
parent resolution, forgotten/duplicate author chains, and a contradictory assertion
after real map operations. The index layer runs these eight plus the author-byte
comparison mutation. The walk layer adds and runs nine mutations for skipping
unseen nodes, losing visited bookkeeping or the initial head, omitting parents,
using a wrong chain, ignoring cache contents, skipping later heads, returning an
empty singleton summary, and a contradictory assertion after nonempty cached
execution. The cache layer adds and runs nine mutations: forgetting the ancestry
predicate, publishing an empty or wrong-head result, omitting snapshot parents,
reading or publishing to the wrong cell, storing the wrong chain, a contradictory
assertion after real initialization, and initializing a false-predicate cell.
The topology layer adds twelve mutations for skipped duplicate rejection, incorrect missing-parent
errors, omitted node/parent/coordinate capacity checks, wrong direct selection,
bypassed ancestral coverage, metadata corruption, reflexive or unknown-hash
ancestry, walking the wrong node, and a contradictory assertion after actual
construction and queries. The head layer adds twelve mutations for missing/incorrect parent removal,
omitted new heads, lost unrelated heads, cardinality/membership guard omissions,
skipped normalization, unconditional shortcut use, missing tips, wrong unknown
hashes, an overly broad head definition, and a contradictory assertion after real
queries. The permission layer adds twelve mutations for accepting unavailable or
rejecting available frontier heads, repeating only the first frontier hash, losing
bound ancestry, omitting a cached author, selecting the wrong author or bound
chain, denying unbounded authors, granting denied authors, bypassing
authorization, accepting unknown selected heads, and a contradictory assertion
after actual permission capture. The rebase adds six mutations for omitted
unlisted-bound initialization, selecting the fallback for a listed author, ignoring
explicit overrides, discarding fallback updates, retaining removed overrides and
granting unlisted authors by default. The capture layer adds twelve mutations for
causal/authorized clock confusion at construction, clone and query, a dropped
clone clock, unknown-hash acceptance, refreshing from current ancestry, a wrong
chain, incorrect captured meaning, missing frozen-tip bounds, weakened extension
rules and a contradictory assertion after real capture and repeated appends.
Eight lineage mutations remove compatibility invariants, admit incompatible
updates, retain or misrecord old receipts, replace lineage during append, capture
a receipt from another lineage, ignore lineage identity or omit its query proof.
Twelve identity mutations weaken the counter invariant, skip exhaustion, fail to
advance the counter, register/return the wrong ID, bind the wrong lineage, register
failed CAS attempts, reset the global allocator, corrupt equality/clone or bypass
either membership guard.
The full 344-case sweep is configured in CI; these targeted runs are local evidence,
not a claim that GitHub Actions successfully executed them.

The ordinary tests compare interval results with an independent linear filter,
exercise zero/unlimited batches and delivery after capture, and retain the
workspace's randomized parent-walk, visibility, persistence, and replica tests.
The [authorization regression tests](../stockfoil/src/history/authorized_visibility.rs)
exercise the policy through `changes`/`next_changes`, including permission
transitions and a frontier becoming known after capture.
These internal raw-traversal tests complement the proofs; they do not discharge
model hypotheses or verify the new public Materialization protocol.

There are 38 Rust/stdlib contracts/declarations and five BLAKE3 declarations,
documented and pinned in [TRUSTED_STDLIB.md](TRUSTED_STDLIB.md) and
[TRUSTED_BLAKE3.md](TRUSTED_BLAKE3.md). Application code contains no project
assumptions or unchecked bodies. The original suite still passes `--no-cheating`. The trusted base includes Verus and erasure, vstd's library
specifications, Z3, and Rust. Allocation failure, resource limits and cryptographic
security are outside this milestone. The proof count includes helpers and loop
obligations; it is not a measure of production coverage.

Current validation also passes 266 workspace tests, 76 trust-audit regressions,
all-features compilation, rustdoc link checking and formatting. The two existing
ignored tests are unchanged. The new mutation cases cover saved-count/binding validation, public resume/plan
inputs, event metadata and payload bindings, acknowledgement feedback, skipped
completion and contradictory API replay callers.

## Next proof obligations

1. Extend public partial-resume replay to rejection/payload feedback and correction
   rounds, repeated live driving, later growth outside the saved capture and the
   subsequent fresh view. Build on the checked codec/resume/plan/acknowledgement
   path, reconstructed-source consumers, pending-checkpoint and counter invariants. Application semantics and atomic external persistence
   remain documented caller obligations.
2. Extend History's invariant through admission, certificate coverage, compaction
   and export. Specify cryptographic, persistence and fairness assumptions when
   proving attestation completion and sync convergence to minimal inventories.

[PUBLIC_API_PROOF_BOUNDARY.md](PUBLIC_API_PROOF_BOUNDARY.md) distinguishes these
open goals from the completed public ancestry contract.
