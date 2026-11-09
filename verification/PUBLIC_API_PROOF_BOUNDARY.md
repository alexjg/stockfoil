# Public API proof boundary

The captured ancestry membership and causal difference APIs are now verified under the accepted
[standard-library assumptions](TRUSTED_STDLIB.md). The harness checks the actual
exported types and method bodies, with no caller validity preconditions. See
[API_TARGET.md](API_TARGET.md) for the complete contract and proof path.

This establishes participation according to the stored graph, recorded authors
and supplied permissions. It does not establish cryptographic authentication,
payload secrecy or correctness of the other public protocols.

## Status of the requested guarantees

| Goal | Established | Remaining library work |
| --- | --- | --- |
| Authorized visibility | Actual public captured ancestry/participation and causal difference; exact captured selection, budgeted events and activation bounds through `plan`, `step` and `update`; local edit guards and acknowledgement; partial resume/plan/unchanged acknowledgement across equivalent reconstructions. | New feedback/correction rounds, repeated live replay, later growth and subsequent fresh views. Admission/authentication is a separate protocol. Inactive records may still be delivered: participation is not payload secrecy. |
| Consistent materialization | Actual public planning, completion, driving and local edits; exact events/order/counts, feedback/token discipline, checkpoint transitions and error handling. Source readiness/ownership, conserved source progress, exact staged checkpoints, distinct-rejection counts and initial-plus-distinct-correction accounting now hold across public API sequences. Public checkpoint serialization preserves all metadata through decode/resume; public partial resume/plan/unchanged completion preserves corresponding events, counts and checkpoint bytes. | Extend reconstruction replay through new feedback, correction rounds, repeated live driving, later growth and fresh views. Application semantics and external atomic storage are caller obligations. |
| Compaction | Supporting graph/clock lemmas. | Prove certificate retirement preserves delivered/admitted history, inventory coverage and export evidence, including canonical fragment containment and `history/admit.rs::add_certificate`. |
| Sync convergence to minimal trees | No end-to-end API proof. | Define minimality over fragment coverage, prove canonical nesting/retirement and export/admission transitions, and state fairness assumptions. Successful delivery alone is not an acknowledgment. |
| Attestation completion | No end-to-end API proof. | `History::sign`, coverage, evidence assembly, admission fixed-point progress and signature assumptions. Complete evidence plus admitted dependencies must admit eligible drafts; incomplete evidence must not. |

Public `History::new`, Default and `is_ancestor` are also checked. The public
permission collection's construction, effective-rule lookup and updates were
already checked; iteration and From/FromIterator conversions remain outside the
harness. Permission compilation accepts arbitrary collection values.

## Public partial-resume replay is now checked

[The API replay milestone](README.md#public-partial-resume-planning-and-acknowledgement-replay)
connects the public checkpoint codec, `Materialization::resume`, `plan` and an
unchanged `complete` report across equivalent rebuilt histories. It proves
corresponding ordered event kinds, hashes, authors, parent membership and visibility,
correct per-ledger payload handles, equal counts and identical published checkpoint
bytes. Existing rejections are supported. Parent-vector order/multiplicity, raw
payload handles and batch tokens are not equated.

New rejection/payload feedback and correction rounds, repeated live driving, later
history growth and the subsequent fresh view remain. Application effects and
external atomic persistence remain caller obligations.

## Reconstructed-source replay is now checked

[The reconstruction milestone](README.md#replay-across-reconstructed-histories)
proves that equal hash-identified parent graphs and authors produce the same
captured authorization and canonical source replay, even with different indices,
chains, History identities and payload handles. Actual constructor/advance/codec
consumers preserve saved progress, logical records and checkpoint bytes. The graph
isomorphism is explicit; reachability and capture equality are derived.

Composition through public planning, event translation and completion, plus later
additions outside the saved capture, remains next. This does not prove admission
or faithful reconstruction by external persistence code.

## Public checkpoint serialization is now checked

The actual public `Checkpoint::to_bytes`/`from_bytes` methods preserve all saved
metadata across a round trip, encode equal checkpoints deterministically and
agree with an exact structural parsing model. A checked caller also carries the
decoded value into `Materialization::resume`, preserving the checkpoint and
establishing the driver invariants. This closes the checkpoint/cursor codec gap
without new project trust assumptions or format changes. Reconstructed-history
and capture-recompilation replay equivalence remain next. See
[the checkpoint milestone](README.md#public-checkpoint-serialization-is-verified)
and [the checked callers](checkpoint_codec_examples.rs).

## Complete cursor serialization is now checked

The actual cursor serialization methods preserve all delivery/view heads and
permission rules across a round trip, produce deterministic bytes, and agree with
an exact malformed-input specification. Permission-map ordering and duplicate-key
rejection are checked too, without new trust assumptions. The cursor type is
internal in normal builds and exported under `fuzzing`; the public checkpoint
codec uses it internally; its outer composition proof is now complete above. See
[the cursor milestone](README.md#complete-cursor-and-permission-map-codecs-are-checked)
and [the checked callers](cursor_codec_examples.rs).

## Checkpoint codec primitives are now checked

The production byte reader, integer/hash-list encoder, permission codec and
checkpoint boolean decoder are now checked, with arbitrary-value round trips,
exact byte consumption, collection bounds and malformed-input rejection. The
public checkpoint/cursor composition through versions, permission/rejection maps,
follow rules and captured progress was completed in the subsequent milestones above. No new trust
assumptions or wire-format changes were introduced. See
[the codec milestone](README.md#checked-checkpoint-codec-primitives) and
[the checked callers](codec_examples.rs).

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

## What the ancestry proof includes

The actual History representation contains a private, invariant-bearing Topology.
Its constructor, hash resolution, chain selection, reserved insertion, delivered
heads, cached ancestry and metadata updates are checked. No caller supplies
correct-cache, chain-selection or compatibility premises to the public queries.
The remaining History fields are represented exactly, without assuming valid
admission/evidence state. Admission flag framing is not an authentication proof.

A checked atomic allocator binds numeric IDs to logical lineages. Captured clocks
carry erased receipts and frozen causal/authorized hash sets. The numeric guard
establishes compatibility, and later checked insertions preserve old answers.
The exported wrappers verify exact Error variants and head normalization,
including clone and the retained redundant ancestor heads.

The harness imports these production bodies directly. A separate pinned manifest
checks the public re-export wiring. This is a review guardrail, not an additional
correctness assumption or a proof of the Rust build system. The nonempty public
proof fixture now constructs its draft through checked public Ledger commit;
identity construction and admission remain outside that publication proof.

## What public difference adds

The exported `Ancestry::difference` returns exactly the difference of the two
frozen causal hash sets, rejecting either wrong originating history with
`Error::WrongHistory`. Its construction derives clock/chain premises from the
checked captures. The exported `Commits` type maintains the joint traversal
invariant, maps each emitted index to the unique stored hash, and proves exact
remaining counts, ancestor-first/minimum-ready-hash order and fused exhaustion.
The standard Iterator model is checked too: an arbitrary public consumer using
`collect::<Vec<_>>()` proves exact, duplicate-free, ancestor-first results.

These public methods have no caller validity premises. The iterator borrows
History immutably, so topology growth during iteration is excluded by Rust's
borrow rules. Captures may predate later deliveries and may be dropped after the
iterator is constructed. Permissions are intentionally ignored by this causal
API. The `History::commits_since` wrapper is now checked as described above.

## What materialization planning and emission add

The actual `plan_changes_at` entry point is checked for arbitrary typed inputs.
It returns exactly new deliveries plus the symmetric difference of authorized
visibility, all within the supplied captured delivery frontier. Unknown input
heads and known-but-uncovered required heads are rejected. Saved delivery heads
and the target view are copied exactly in their logical representation. Public
view/cursor constructors, accessors and clones, permission clones, and
`Changes::remaining` are checked too. Codecs remain outside the proof.

The actual emitter appends exactly `min(max, remaining)` records while preserving
any existing output prefix. Each record names the stored payload ID for its hash,
marks delivery relative to the prior captured delivery set, and classifies the
frozen before/after authorized sets. Selection and those membership sets survive
History growth. Order is ancestor-first with the smallest currently eligible
hash; records are removed exactly once. Completion occurs exactly at exhaustion
and returns the saved delivery sequence and target policy. A wrong History leaves
both traversal and output unchanged, including with zero budget or exhaustion.

Payload correspondence is to the retained node at emission; the frozen receipt
does not independently snapshot payload bytes or prove payload confidentiality.
Denial prevents participation, not delivery of metadata. Generic checked consumers
compose planning and complete draining; concrete consumers emit a denied delivery
and a retraction after growth. This is the internal boundary used by
Materialization. Its public asynchronous lifecycle and application publication
remain separate obligations.

## What immutable batches and the handshake add

The actual public `Batch` accessors and clone preserve its full logical event
sequence, including ordered parents, payload IDs and activity. An erased issuance
receipt binds that sequence to a planner and ticket. The production pending slot
rejects overwrite and ticket exhaustion without mutation, returns the exact
supplied events on issuance, and accepts completion only for its current token.
Successful consumption returns the exact stored pending value and empties the
slot. Stale/foreign tokens leave it unchanged. All clones identify the same work;
later follow-ups use strictly newer tickets. Planner identities now use the
checked non-reusing allocator rather than a wrapping counter.

The pending value is generic. These results do not assume or establish correctness
of the proposed checkpoint inside it. The enclosing Materialization methods,
rejection/cascade logic and application publication are not yet included in the
harness. `Batch::resolve` and generic Event mapping remain unchecked too. The
production delegation is guarded by the source manifest; that guard is not a
proof of the enclosing method bodies.

## What checkpoint values and transitions add

Public Checkpoint construction, defaults, clone, follow/heads/partial accessors
and Follow copies are checked. The actual staging helper builds pending metadata
without altering the published checkpoint: it either retains the supplied base
plus exact target/frontier/count, or installs the supplied completion cursor and
clears partial progress. Both preserve follow and rejections. Restart empties the
cursor and resets the count while keeping the captured target/frontier and
permanent rejection state. Rejection insertion preserves unrelated entries and
all other metadata, never weakens a cascade, and returns exact newness.

Checked consumers connect real planning/emission to staging and real pending
checkpoint values to the token handshake. They do not prove that the enclosing
Materialization source always supplies the correct processed count or completed
cursor, or that application reports and correction rounds justify publication.
The Source milestone below supplies the count and completion link for a captured
source. The outer lifecycle still needs composition. Checkpoint codecs and the
public rejection iterator remain outside the harness.

## What the captured source adds

The actual Source constructor checks saved progress and skips exactly that many
canonical records in bounded chunks. Canonical prefix uniqueness establishes
that chunking cannot change the selected suffix; a generic consumer proves
resumed and uninterrupted sources agree for the same History snapshot.
Advancement maintains processed-plus-remaining equal to the captured total,
reserves a slot for a pending Reset, and emits exactly the permitted number of
records without count overflow. Captured metadata and membership survive History
growth. Wrong-history errors leave the source unchanged.

Checkpoint preparation now derives its count and completion cursor from maintained
source state. After successful advancement, completion means exactly exhaustion
and publishes the saved target/frontier in the proposed checkpoint. An unwind
during owned advancement leaves a source that rejects further use with `Poisoned`.
These are internal Source guarantees. Translation is checked as described below;
report and correction handling are checked as described below; final checkpoint publication remains outside the proof.
The same-snapshot replay proof does not establish graph-reconstruction equivalence
or equality of captures independently recompiled after History growth.

## Record-to-event translation and rejection filtering

The actual production translator now specifies the complete ordered event sequence,
including an optional leading Reset, inactive structural deliveries and suppressed
old-record transitions. It reads author and ordered parents from actual topology
storage and preserves source payload IDs. Direct rejection and cascade-root strict
ancestry are checked against the checkpoint and real History query. Its summary
pass proves exact event counts, delivered hashes and active-new-delivery order.

A checked consumer advances the actual captured Source, translates its records,
stages its checkpoint and issues a real Batch token. Every active Deliver or
Activate in that token is in the frozen authorized set and passes rejection
filtering. No application-state behavior is assumed or proved. The internal
known-record premise is discharged from Source emission in this consumer; the
public planning path and completion are now checked separately below; public
`step` and `update` compose them as described above. Application persistence still
requires its own proof. Inactive metadata delivery is intentional, so this is a participation
property, not payload confidentiality. Report validation is checked as described
below; public `complete` and `step` now check follow-up handling and checkpoint publication.
Concrete application/durable-storage correctness remains a separate obligation.

## Report admission and rejection recording

The real completion path now checks the batch receipt, rejection membership and
uniqueness, and wanted-hash existence before consuming pending work. Returned
errors preserve the entire Protocol state. Error precedence is explicit: stale
token, then malformed rejection report, then unknown wanted hash. Success moves
the exact pending value, preserves all rejection fields, deduplicates the exact
wanted set and consumes the token once. The permission to reject refers to the
pending original source batch, retained across immediate follow-ups; wanted hashes
are checked for existence, not batch membership or authorization.

Public Applied default/want and initial Pending construction are checked. The
rejection-recording helper preserves all checkpoint fields except the exact merged
rejection map, never weakens an existing cascade, and counts only distinct hashes
absent from the old map. It requires no validity premise, even for direct duplicate
input. Consumers compose this with actual emission, translation, pending construction
and token issuance, and prove retry after invalid feedback. They do not prove the
outer Materialization method on their own. Public completion is now checked below;
application effects, persistence and recovery after consumed-token failures remain open. Correction generation and restart are
checked below. The generic rejection builder's reason conversion and derived report clones are not checked here.

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

The history-binding guard is checked below. Cumulative counter additions,
and final application-state/checkpoint publication remain unverified. These are
normal-return guarantees; allocation failure, unwind recovery and application
behavior are not proved. Compaction, sync minimality, attestation completion and
replay equivalence across reconstructed graphs remain separate open goals.

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

## Next proof obligations

1. Extend public partial-resume replay through new feedback/correction rounds,
   repeated live driving, later growth outside the saved capture and fresh views,
   building on the checked public codec/resume/plan/acknowledgement path. Keep
   application semantics and atomic external persistence explicit as caller obligations.
2. Extend History's invariant through admission, certificates, compaction and
   export transitions for the remaining three requested guarantees. State hashing,
   signatures, host persistence and distributed fairness assumptions explicitly.

## Trusted boundary and failure behavior

The full suite uses 38 enumerated Rust/stdlib contracts and five approved BLAKE3 declarations; the original subset also
runs under Verus's `--no-cheating`. Application logic cannot add unchecked bodies
or axioms. Library contracts cover collection behavior, predicate-preserving
OnceLock access, atomic/static access, capacity, head normalization/sharing, fixed-key ready-set removal and lazy map-range iteration.
They do not assume correct Stockfoil summaries, permissions or identity bindings.
The initializer, allocator and application composition prove those properties.

The guarantees apply on normal return. The checked Topology publication path
preserves its invariant across unwinding, using fallible reservations before
logical mutation and a non-unwinding commit. The public difference iterator and internal owned cursor take
their cursor out before advancing; an unwind leaves a valid exhausted iterator.
Normal steps restore the cursor and satisfy the exact selection/count contract. Standard abort-on-OOM behavior is
required for infallible B-tree allocation. Successful allocation, starvation
freedom and recovery of the rest of History's publication protocol are not proved.
