# Verified public API boundaries

Work continues on `verus/authorized-api`, rebased onto `main` at
`f2d470eeae4d11fe7bd7d7f65d9d88bcd8f0a65d`. The previous PR stack is historical.
Public captured ancestry membership and causal difference iteration are verified under the
[enumerated standard-library assumptions](TRUSTED_STDLIB.md).
The public immutable `Batch` accessors and clone are now checked too, together
with the internal issuance/consumption handshake used by Materialization.
Public `Checkpoint` construction, clone, `follow`, `heads` and `is_partial` are
also checked, along with the production checkpoint staging/restart/rejection helpers.
Public Materialization construction/resume, checkpoint/follow/heads/dirty accessors and
`set_follow` are now checked, together with its production history-binding guard.

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

The user corrected the hashing choice to **trust BLAKE3's implementation**. That
approval is now embodied in a small adapter with five explicit declarations for
opaque state, initialization, clone, append and finalization. The verifier links
the same pinned real BLAKE3 crate as production. See
[TRUSTED_BLAKE3.md](TRUSTED_BLAKE3.md) for the exact scope and dependency pins.
These are assumptions, not proofs of BLAKE3 or cryptographic security.

The actual public `History::begin_commit` and `Ledger::begin_commit` bodies now
verify parent validation/selection and construction of a builder with no payload
bytes. Public builder cloning preserves metadata and streamed bytes. Each update
appends exactly its supplied slice. Finish returns the exact selected author and
ordered parent sequence with identity
`BLAKE3(J(parent_count) || parents || author || BLAKE3(payload))`. The actual
parent-count encoder is checked against bijou64's tiers, offsets, tag and
big-endian payload. The outer streaming loop checks each parent, including
repetitions, followed by author and payload digest. No `bijoux` assumption was
added, and the existing wire encoding and identity fixtures stay unchanged.

The actual public `Document::commit` body now invokes the supplied native Into
conversion, prepares metadata, hashes the converted bytes, registers that exact
shared payload and publishes through the checked Ledger. Its contract relates the
result to the **actual conversion's** postcondition, rather than assuming arbitrary
user conversions preserve a separate byte interpretation. Preparation errors leave
the document unchanged. Later publication errors may retain registered bytes, and
duplicates retain the original slot and stored metadata as before.

The actual public `Materialization::change` body now connects every phase: checked
preflight and the native edit callback; exact Vec-to-Arc byte preservation; the
completed checkpoint's exact parent sequence; public Document commit; and checked
local acknowledgement. Its closed public outcome records the complete phase trace.
The only callback precondition is the actual closure's own precondition when
preflight admits it; there is no assumed application-state law or blanket caller
validity premise. `Ok(Some(draft))` additionally ensures the driver is current and
dirty. The draft is not echoed through apply, and change does not checkpoint it.

Preflight failures do not run the edit or change application state. `Ok(None)` may
retain application mutation without publishing. Edit errors retain their exact
value and poison the driver. Publication or acknowledgement errors also poison
it after editing; neither document nor application rollback is promised. Hash
collisions/freshness are not assumed away: duplicate publication can succeed while
acknowledgement fails. A successful result therefore does not assert that every
possible edit succeeds, or that arbitrary application state matches the draft.

The codec and Display bounds that the new bodies do not use were relaxed. Existing
concrete calls remain valid; the algorithms are checked for arbitrary untouched
codec/options/queue fields and preserve the public error types.

[builder_examples.rs](builder_examples.rs) proves public streaming/clone identity,
default-builder success and a nonempty public Document commit constructed from
actual empty values. [change_examples.rs](change_examples.rs) proves actual public
change rejection, no-commit success and exact edit errors through checked native
callback contracts. Runtime regressions cover independent hash clones, encoding
across every tier and the u64 range, custom Into conversion, unchanged wire
identities, and successful local changes without echoed application.

The active boundary is **38 Rust/standard-library contracts plus 5 BLAKE3
declarations**. The added standard-library contract is only Vec<u8>-to-Arc<[u8]>
byte preservation. No application correctness, collision resistance or signature
assumption was added. These proofs cover the API protocol on normal return;
concrete application-state/checkpoint correspondence, durable persistence, general
lifecycle reachability, replay across reconstructed histories, compaction, sync
minimality and attestation completion remain separate open goals.

The commit-construction milestone rejected all 33 of its new semantic mutations;
its configured suite then contained 967 cases. Its complete sweep was not rerun locally.

## Commit preparation now preserves checked metadata

The sections below record preceding milestones. Their historical validation and
trust counts describe those stages; the active boundary is the commit/change
composition above.

Both public `History::begin_commit` and `Ledger::begin_commit` now use checked
metadata preparation before initializing the existing BLAKE3 builder. History
preserves the requested author and exact parent sequence, rejecting the first
missing parent. Ledger uses its actual local author. `Some(parents)` retains every
parent in order, including repetitions; `Some([])` starts a root. `None` captures
all delivered heads, including drafts, in the existing BTree order without
duplicates. A checked topology lemma derives that every delivered head is known,
so default preparation always succeeds on normal return.

The actual `CommitBuilder` stores the new private `CommitMetadata` value instead
of separate author/parent fields. Its checked accessors and clone preserve those
fields exactly. After the existing hash calculation, a checked consuming method
attaches the supplied digest while preserving author and ordered parents in the
actual `Prepared`. This contract accepts any digest and does not assume that it
matches the metadata or payload. Later history growth does not change the owned
metadata. No public signatures or commit encoding were changed.

The actual public `Prepared::signing_bytes` body is now checked too: it returns
exactly the current hash's 32 bytes. Since `Prepared.hash` remains publicly mutable,
this is deliberately a statement about the current value, not authentication of
the original preparation. A checked caller changes that hash and proves the
returned signing bytes follow it.

[metadata_examples.rs](metadata_examples.rs) checks arbitrary preparation results,
clone/finish preservation, default-parent success and explicit empty roots. A
constructed example prepares and publishes a root through actual checked methods,
then selects default, repeated and empty parent lists. Runtime public API tests
cover captured parents across later history growth, clone and chunking identity,
exact parent order/repetitions, and first-missing-parent errors without history
changes. The existing wire and commit identity fixtures remain unchanged.

The public builder, hash-input construction and commit/change wrappers are now
checked by the newer composition above under the approved BLAKE3 boundary.

## Local edit phases now check callback effects and poisoning

The actual public `Materialization::change` now delegates to two checked production
helpers, `begin_edit` and `finish_edit`, around its existing Document commit call.
The wrapper and hash-input construction are now checked by the newer composition
above; this section describes the underlying edit-phase contracts.

`begin_edit` checks history binding first, then currentness, then Signed follow,
then the local author's effective write permission. Only Unbounded permits an
edit: an explicit rule overrides the unlisted-author fallback as usual. Each
failure preserves application state and all driver fields except successful
initial history binding, without newly poisoning the driver. The contract captures
the exact error precedence, including stale state preceding Signed rejection.

After those gates it invokes the actual supplied `FnOnce(&mut M)` callback. The
contract records the initial/final application state and original callback result
through Verus's native `call_ensures`; it does not introduce a separate assumed
application law. A verified caller must establish the callback's own preconditions
only when the gates admit the call. A rejected-caller proof even uses a callback
with an impossible precondition, proving it cannot be invoked on that path.
Normal-return guarantees exclude callback termination, panic recovery and rollback.

Returned bytes are preserved exactly. `Ok(None)` and `Ok(Some(bytes))` preserve the
driver apart from history binding, including its existing dirty flag, source,
checkpoint, protocol and counters. `None` does **not** assert that the application
state is unchanged: the caller's callback may mutate it without producing a commit.
An edit error is returned unchanged as `DriveError::Edit`, preserves those driver
fields and sets poisoned. No materializer apply/checkpoint callback is invoked.

`finish_edit` propagates a Document publication error unchanged and poisons the
driver. On a published Draft it calls the checked public `mark_local`; success
returns exactly that Draft and inherits its current/dirty, metadata, authorization
and checkpoint guarantees. A failed acknowledgement preserves its exact error and
then poisons the driver. This phase does not undo the edit or document publication.
The failure may follow a duplicate publication that succeeds but provides no sole
new delivery for acknowledgement.

The checked parent-validation loop from public publication is now also used by
`History::begin_commit`, preserving first-missing-parent precedence and parent
order/repetitions. The newer builder proof composes this loop with checked hashing-input construction
to cover the public preparation API.

[edit_examples.rs](edit_examples.rs) checks arbitrary rejection, success with no
bytes, exact bytes, exact edit errors, publication failures and acknowledgement.
A ready fixture is constructed from actual empty History, captured delivery heads,
policy and checkpoint values, rather than assuming readiness. Checked callback
implementations prove observable application effects. Runtime public `change`
tests cover one invocation, mutation followed by None/error, poison blocking retry,
stale-before-Signed precedence, and failed acknowledgement of a duplicate after
editing. The existing successful change test still checks no echoed application.

The next gap is preparation/hash construction and its composition with these
phases and verified payload publication in the public wrapper. The trusted boundary
remains 37 library/Rust contracts. General lifecycle reachability, concrete
application-state/checkpoint correspondence and durable storage remain open;
compaction, sync minimality and attestation completion remain separate goals.

## Public prepared-commit publication is now checked

The harness imports the actual `Prepared` and `Delivered` representations and the
exported `History::publish_commit`, `Ledger::commit`, `History::epoch` and
`Ledger::author` bodies. There is no new application or library assumption.
`Prepared` has no assumed hash/authenticity invariant; its hash remains publicly
mutable. The contract concerns publication of the supplied metadata.

For a new hash, History checks parents in their supplied order, returns the first
missing parent, then checks epoch exhaustion and topology capacity. Success stores
exact author, ordered parents including repetitions, payload ID and leading-zero
byte depth; adds the node to drafts; advances epoch once; and preserves admitted
heads, signatures, signed drafts and inventory. Existing nodes and parent storage
remain intact, the causal state extends the old state, and history identity and
capture lineage are preserved. The existing topology proof supplies exact head
updates and validity. The shared executable depth calculation is now checked too.

A duplicate succeeds before parent or capacity checks, changes nothing and returns
the original payload ID. Submitted metadata need not match the existing record.
Ledger first rejects a different local author, then delegates publication while
preserving options, queue and policy. Its successful Draft describes the **submitted**
metadata, even for a conflicting duplicate; it does not assert that this Draft
matches storage. The separately checked `mark_local` validates that correspondence.
All returned History/Ledger publication errors preserve their entire prior value.

Epoch and topology-capacity exhaustion now return `Error::Exhausted` instead of
panicking. In particular, epoch exhaustion is detected before inserting a node.
This is a normal-return guarantee; allocation success and unwind recovery are not
proved, and the unchanged admission/signing paths retain their own counter logic.

The actual internal `Document::publish_prepared` helper is also checked. A new hash
registers the exact bytes at the previous vector length and records that payload
ID; an existing hash reuses the original slot and bytes. Registration happens
before Ledger publication, so a later error retains registered bytes and the index
entry while preserving Ledger. This matches the previous public path. Its contract
does not assume index completeness, payload availability or a hash/bytes identity
relation. A defensive payload-ID exhaustion check preserves the whole Document.

[publication_examples.rs](publication_examples.rs) checks arbitrary callers,
wrong-author rejection and successful root/child publication with repeated parents
and duplicate payload retention. [local_examples.rs](local_examples.rs) now obtains
its nonempty Draft from public `Ledger::commit`, then proves `mark_local` makes the
pin current and calls the real checkpoint hook without applying the draft again.
Runtime tests cover epoch boundaries, error precedence, duplicate metadata and
bytes, retries after failed publication, and every leading-zero depth position.

`CommitBuilder`, hash-input construction, `Document::commit` and public `change`
are now checked above. Next connect a checked application's state to its checkpoint. General lifecycle
reachability, durable storage, compaction, sync minimality and attestation
completion remain separate open obligations.

## Public `mark_local` now checks local acknowledgement

The actual exported `Materialization::mark_local` body is checked for arbitrary
typed driver, Ledger and Draft values, without caller validity preconditions.
The actual Draft representation is shared with the harness; it carries no assumed
publication or authenticity invariant. The method acknowledges application work
that the caller says has already happened. It neither applies events nor proves
that the application actually reflects the draft.

The acceptance rule is precise: history binding must succeed, no batch or partial
checkpoint may be outstanding, and the follow rule must not be Signed. The draft's
hash must name a stored record with the exact author and ordered parent sequence,
including repetitions. Separately, its parent **set** must equal the checkpoint's
completed-head set, current permissions must equal the saved policy, and the draft
must be the only delivered commit outside the causal past of saved delivery heads.
Those heads must exist. Finally, the draft must participate in the ancestry rooted
at itself under current permissions and pass both direct and cascade rejection.

Success advances delivery progress to the current delivered heads, installs the
view selected by the follow rule and current policy, clears the old source, and
marks the checkpoint dirty. Working remains Working; a pin moves to exactly the
new draft. The driver is current and unpoisoned, has no pending batch or partial
checkpoint, and preserves rejection records, protocol identity/tickets, support
flags and last-step counts. No materializer callback occurs in acknowledgement.
A subsequent ordinary step can checkpoint that already-applied state without
redelivering the draft.

Errors follow the production guard order: binding, outstanding/partial work,
signed-only view, metadata mismatch, unknown delivery heads, stale parent/delivery/
policy state, then nonparticipation or rejection. Every error preserves the entire
driver apart from successful initial history binding. It does not newly poison
the driver or consume a pending token. The successful-caller proof establishes
that satisfying all acceptance conditions returns Ok, not merely that Ok implies
them. These are normal-return guarantees; panic recovery is not claimed.

The supporting public `History::commits_since` wrapper is now checked too. It
returns all stored commits outside the supplied heads' causal past, irrespective
of permissions, or an actual unknown-head error. Its real Commits iterator retains
exact size, no duplicates and canonical ancestor-first order. Public Ledger
`commits_since`, `ancestry`, `author_of` and `parents` now have checked bodies and
contracts. Parent queries preserve order and repetitions; missing metadata returns
None. These query proofs establish stored metadata, not cryptographic authorship.

[local_examples.rs](local_examples.rs) checks arbitrary outcomes, acceptance,
poisoning precedence and collection through the standard iterator. A nonempty
fixture calls public Ledger commit, acknowledges its actual draft, proves the
pin current and invokes the real checkpoint hook without an apply callback.
Runtime regressions cover exact/repeated metadata, changed policy, additional
deliveries, denied and signed views, pending tokens, partial progress, retry after
errors, and `mark_local` followed by an update with no repeated delivery. Existing
concrete-Ledger function-pointer calls remain valid.

Next is the complete `change` wrapper and its commit-preparation dependency.
`Ledger::commit` is now checked above. `Document::commit` preparation and admission
are not assumed correct and remain outside this proof. General lifecycle reachability, concrete application-state/
checkpoint correspondence and durable storage remain open, as do compaction,
sync minimality and attestation completion. The trusted boundary stays at 37
library/Rust contracts, with no new application assumptions.

## Public `update` now composes verified steps

The actual exported `Materialization::update` body is checked for arbitrary typed
Document, driver and Materializer values, with no caller validity preconditions.
It calls the checked public `step` with budget 1024 until Current or an error.
An erased finite trace connects the exact driver and application state between
every successful More step and the final call. Each edge uses the actual step
contract, including its callback arguments, feedback, captured activation bound,
checkpoint hook and error handling. No application behavior is assumed.

On success the driver is current, clean, unpoisoned and has no pending batch.
`Progress::heads_before` is exactly the input checkpoint's completed-head sequence;
`heads_after` is exactly the final checkpoint's sequence. All four counters are
the mathematical sums of the completed steps' counters, including the final
Current step. `rebuilt` is their logical OR. Counts describe event activity,
including replay, rather than unique commits or a difference between head sets.
Public Progress Default, Clone and `changed` are checked too; a checkpoint-only
update can report no activity.

The previous unchecked counter additions now use checked arithmetic. Aggregation
computes every sum before changing the report. If any total exceeds `usize::MAX`,
update returns `DriveError::Stockfoil(Error::Exhausted("materialization progress"))`.
This happens **after a successful step**: its checkpoint and application effects
remain, its driver is clean and unpoisoned, and no total is silently wrapped or
saturated. The method returns no partial Progress on an error.

If a step itself fails, update returns that exact error and preserves that step's
recovery boundary. Earlier successful steps and checkpoints remain completed.
The failing step restores its entry checkpoint and poisons after successful
preflight; preflight failures make no application calls. Application mutations
from a failing callback are not rolled back. Restoring application state and the
matching checkpoint remains the application's responsibility.

The proof strengthens `step` to establish that More strictly advances its ticket
sequence. Update's loop decreases the remaining ticket range; the checked caller
also bounds the number of preceding More steps by that range. This proves driver
loop progress when callbacks return, not callback termination or panic recovery.
The explicit boundary is still 37 library/Rust contracts, with no application axioms.

[update_examples.rs](update_examples.rs) checks arbitrary public outcomes, exact
aggregate totals and error boundaries, ticket progress, and concrete arithmetic,
clone, no-op and overflow witnesses. Runtime regressions cross the 1024-record
budget with 1025 commits, include an immediate payload follow-up, check both
partial and final checkpoint hooks, and confirm a clean-current no-op. A later
apply failure leaves the earlier partial checkpoint usable after application
restoration. Separate tests check atomic overflow in each counter and rebuild
accumulation. Named `update::<Codec, State>` and function-pointer calls remain valid.

Next are `change` and commit preparation, general lifecycle reachability and a checked
application's state/checkpoint correspondence. Durable storage, compaction, sync
minimality and attestation completion remain open goals.

## Public `step` now checks the in-memory driving protocol

The actual exported `Materialization::step` body is checked for arbitrary typed
Document, driver and Materializer values, without caller validity preconditions.
The proof uses the real Materializer trait, Step and DriveError types. Existing
named `step::<Codec, State>` calls and concrete function-pointer uses remain valid;
reads are parametric in Document's untouched codec/receive/queue fields.

Preflight checks history binding, positive budget and outstanding work in that
order. Errors here make no application calls and preserve the driver apart from
successful initial history binding. A successful preflight resets the last-step
counts, then invokes the checked public planner with the supplied budget.

Each source/follow-up round resolves the whole batch before calling the actual
`Materializer::apply` method with that Document's History and those exact byte
events. Its actual report is passed to public `complete`. An erased finite round
relation connects the application state and planner state across every call and
returned report. All callback activations belong to the first batch's activation
set; that set lies inside the captured authorization and outside direct/cascade
rejection. Completion follow-ups cannot add activations. Inactive deliveries may
still carry metadata and bytes: this remains visibility control, not secrecy.

Only after all follow-ups finish does the driver call the actual checkpoint hook,
with the completed checkpoint, and only if it planned work or was already dirty.
Successful checkpointing clears dirty; a clean current driver makes neither
callback. On success no token remains pending, and Step::Current is returned
exactly when the checked `is_current` predicate holds. Otherwise it returns More.

A returned error after preflight restores the original checkpoint's logical value
and poisons the driver. Apply and checkpoint errors retain their original generic
value and distinct phase. The rest of the planner remains at its failed transition:
there is no promise to restore consumed tokens or source progress. The application
can remain mutated, including after a failed checkpoint hook. Preflight errors do
not newly poison it. The feedback loop has a checked decreasing measure: every
follow-up consumes a fresh ticket, and the last round removes the pending batch.
It finishes or returns an exhaustion error when callbacks return. Termination of
application callbacks and panic/unwind recovery are not proved.

Application behavior is not an axiom. The callback relation uses Verus's contract
for the actual invoked method, including a checked implementation's stronger
postconditions. The trait adds no general application-effect or durability law;
its optional default checkpoint body is checked separately. The production Error
Display bound is unused by driving and therefore contributes no proof assumption.
The explicit trusted boundary remains 37 library contracts.

[step_examples.rs](step_examples.rs) checks arbitrary public outcomes, preflight
framing, exact errors from a checked materializer that mutates before failing,
absence of apply calls when no batch exists, and checkpoint success/failure on a
current driver. The nonempty public plan/complete witness now also invokes public
step to checkpoint its completed view. Runtime regressions exercise repeated
payload feedback before a single checkpoint, clean-current no-ops, function-pointer
compatibility, and an apply failure after an earlier successful round.

Remaining: `change` and commit preparation, general lifecycle reachability, and proofs tying a concrete
application's effects and durable storage to the checkpoint. Compaction, sync
minimality and attestation completion remain separate open goals.

## Payload resolution now preserves the verified event envelope

The actual public `Event::map_payload`, `Event::try_map_payload` and
`Batch::resolve` bodies now run in the harness. Mapping preserves each variant,
hash, author, parent sequence and `active` flag. Batch resolution preserves event
length and order without consuming or changing the completion token. Payload
results satisfy the supplied callback contract; fallible event mapping returns
its original error value. The proof requires callback preconditions only for
payloads present in the input. It does not verify arbitrary callback side effects.

The harness now contains the actual `Document` representation and its public
`ledger`/`history` accessors. Codec, receive options, queue, indexes and staged
bundles retain their production representation; byte reads assume nothing about
those untouched fields. Like Ledger, Document uses default type parameters for
its real receive options and queue, letting checked reads be generic over them.
Construction and mutation still require the production Codec implementation and
remain outside the proof.

The checked internal `payload_by_id` returns exactly the stored slot, or
`Error::Storage` exactly when the full ID is out of range. It checks before casting
to `usize`, preventing large IDs from wrapping to another slot on narrower
platforms. No premise claims that raw Ledger payload IDs exist in Document.
`resolve_batch` succeeds exactly when all referenced slots exist, preserving every
event and resolving both deliveries and requested payloads to their stored bytes.
A failure returns an error, never a partially resolved event vector. The actual
`step` now calls this shared checked resolver before invoking the application.

[resolution_examples.rs](resolution_examples.rs) composes public `plan` with
byte resolution and proves that resulting activations remain in the captured
authorized set and outside direct/cascade rejection. Concrete nonempty and
missing-storage witnesses check exact fields, a hidden delivery, repeated payload
references, token preservation, and successful token consumption after resolution.
Runtime regressions cover stateful resolvers, non-Clone callback captures/errors,
and a missing payload after a valid delivery: the application receives no batch,
the saved checkpoint remains published, and the driver becomes poisoned.

This closes the payload-conversion boundary. Public `step` now composes it with
callback sequencing and checkpoint/error recovery as described above. Public `update` now adds exact cumulative progress. General lifecycle invariants
and local edits remain next. There are no new application axioms or explicit
library assumptions. Slot identity is not cryptographic payload authentication or
payload secrecy; inactive deliveries may still carry bytes.

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
boundary remains 37 library/Rust contracts, with no new application assumptions.

## Contract in plain words

`History::ancestry(heads, permissions)` captures the selected heads and their
parent-edge ancestry. It rejects an unknown selected head with
`Error::UnknownCommit` carrying an actual unknown hash from the request. On
success, every selected head is known, and `Ancestry::heads` returns exactly the
requested set, sorted by hash bytes with duplicates removed. Redundant ancestor
heads remain in that set.

`Ancestry::contains` returns true exactly for hashes in that captured causal past.
`Ancestry::participates` returns true exactly for those hashes whose recorded
author's captured permission permits participation:

- Explicit author entries override the fallback, which is denied by default.
- Denied permits nothing; unbounded permits that author's selected commits.
- A bounded permission permits ancestors of its available frontier heads,
  including the frontier itself. Only frontier heads inside the selected past
  grant permission. Unknown and out-of-selection heads grant nothing.
- A frontier may have a different author from the commits it authorizes.

Both queries reject a different originating history with exactly
`Error::WrongHistory`, before resolving the queried hash. On the originating
history, unknown hashes and later commits return false. Later deliveries,
permission changes and newly available frontier heads do not refresh a capture.
Cloning preserves its heads, identity and frozen answers.

The public `History::is_ancestor` method is checked too: it returns true exactly
for strict parent-edge ancestry between known hashes. `History::new` and Default
establish an empty topology and empty auxiliary collections.

`after.difference(&before, &history)` returns an iterator over exactly the hashes
in the first captured causal past but not the second. Either capture from another
history yields `Error::WrongHistory`, including when both histories have identical
contents. Permissions on either handle do not affect this causal difference.
Denied commits and drafts are included when causally selected; later deliveries
cannot expand the old captures.

The iterator emits each selected hash exactly once. Ancestors precede descendants;
among currently eligible records, it chooses the smallest hash. `len()` and both
bounds of `size_hint()` equal the number of remaining hashes. Each successful
`next()` removes exactly its returned hash; `None` means true exhaustion and remains
`None` on further calls. These properties compose through the standard Iterator
model: a checked public consumer uses ordinary `collect::<Vec<_>>()` to prove the
exact set, no duplicates and ancestor-first order. The iterator borrows History,
not the captures, and retains intervals/frontiers rather than an expanded hash list.

## How the public proof is connected

The harness includes the **actual production History, Ancestry, Commits and Error types**
and their methods from [`verified/history.rs`](../stockfoil/src/verified/history.rs),
[`verified/commits.rs`](../stockfoil/src/verified/commits.rs) and
[`verified/api_error.rs`](../stockfoil/src/verified/api_error.rs). Existing
public paths re-export those types. There is no substitute History, trusted
application accessor or assumed authorization result. The actual auxiliary
History data types are included without any admission or cryptographic invariant.

1. Private Topology storage has a checked type invariant covering parent storage,
   indexes, single-author ancestral chains, complete tables, cache bindings,
   delivered heads and lineage. Construction establishes it; checked insertion
   and metadata updates preserve it, including across panic unwinding. Preparation
   reserves capacity before logical changes; joint publication is checked
   `no_unwind`.
2. The real cached walk and head accumulation prove exact parent-edge ancestry.
   Permission compilation resolves actual hashes and author rules and refines
   the independent graph policy. Neither calculation assumes correct summaries.
3. Captured clocks carry erased frozen hash sets and lineage receipts. A checked
   atomic allocator binds numeric history IDs to lineages. The runtime ID guard
   establishes compatibility with the current topology; queries derive the old
   answer using its current hash index.
4. The exported wrappers check the real error mapping, head copying,
   normalization, shared slice conversion and clone. Ancestry has a checked type
   invariant for its stored heads. Public capture and membership methods have
   **no caller validity preconditions** and accept arbitrary borrowed values.
5. [`public_api_examples.rs`](public_api_examples.rs) composes those public
   methods with arbitrary arguments and a nonempty fixture. The latter constructs
   nodes through the checked private insertion path; it is not a proof of public
   commit publication. A source-hash guard pins the production re-exports in
   [`public_api_wiring.json`](public_api_wiring.json), separately from the library
   assumptions.
6. `Ancestry::difference` derives causal-clock bounds from the capture invariant
   and lineage guard. The complete checked constructor builds intervals, pending
   frontiers, queues and exact counts. `Commits` maintains their joint type
   invariant and proves hash/index projection. A pure canonical sequence model
   gives the real Iterator implementation its standard vstd contract without
   executing an eager traversal. [`public_difference_examples.rs`](public_difference_examples.rs)
   checks arbitrary public construction/collection, direct steps, both identity
   guards and a nonempty actual hash emission. No public caller assumes validity.

## Validation and trusted boundary

The current baseline is **212 strict checks and 1,053 full-suite checks**, with zero
errors; these counts overlap. The boundary is **38 explicit Rust/stdlib contracts
plus five approved BLAKE3 declarations**. No application correctness or collision
resistance assumption was introduced. The pure canonical-order model proves its
own bytewise order properties; standard collection/iterator behavior uses the
pinned vstd contracts.

All 266 workspace tests and 76 audit regressions pass; the existing large streaming
regression and one doctest remain ignored. All-features compilation, rustdoc links
and formatting also pass. The configured mutation suite now has 1,124 cases; the
new cases exercise resume validation, event metadata, payload bindings, actual
planning/acknowledgement calls and contradictory public replay consumers. The complete
mutation sweep was not rerun locally. See [README.md](README.md).

These are correctness guarantees on normal return under the documented Rust,
Verus/vstd, solver and library assumptions. They do not guarantee successful
allocation or progress of the atomic allocator under contention. Infallible
B-tree allocation uses standard abort-on-OOM behavior; custom panic-on-OOM
configurations are excluded. `Commits` takes its cursor into an owned local before
advancing; an unwind leaves a valid exhausted iterator. Successful steps restore
the cursor and preserve the public contract. No claim is made about cryptographic authenticity
of the recorded author, payload confidentiality, or admission validity.

## Earlier materialization milestones

These stages are retained as implementation history. The current boundary and
remaining goals are described above.

The internal owned materialization cursor maintains its captured selection,
scheduler invariant, canonical order and exact counts as History grows. The
actual `plan_changes_at` entry point now derives all clock-input premises and
proves exact delivery/visibility selection, captured-frontier coverage, error
mapping and saved target/delivery metadata without caller validity premises.
Public `ViewPoint`/`MaterializationCursor` construction, accessors and cloning,
permission cloning and `Changes::remaining` are also checked; their codecs remain
outside the harness.

The actual `emit_next_changes` body is also checked without caller validity
premises. It appends exactly the budget-limited captured selection, with correct
stored payload IDs, new-delivery flags and frozen before/after visibility. It
preserves the output prefix, canonical causal order and exact remaining counts;
completion returns the saved delivery frontier and target. Wrong histories leave
both output and traversal unchanged, including at zero budget and after exhaustion.

The actual public `Batch::{events,is_rebuild,len,is_empty}` and clone now preserve
the exact logical event sequence, including parent order, payload IDs and activity.
An erased persistent receipt binds those events to the planner and ticket. The
production pending-work handshake checks that binding, rejects stale or foreign
tokens without mutation, and consumes the matching pending value exactly once.
Clones identify the same work; a follow-up receives a strictly later ticket.
Pending work cannot be overwritten, and ticket exhaustion preserves the slot.
Planner IDs now use the checked non-reusing allocator instead of wrapping.

The actual checkpoint helpers now preserve all follow, cursor, rejection and
partial-progress data. Partial staging retains the supplied base cursor and
records the exact captured target/frontier/count; completed staging installs the
supplied completed cursor and clears partial progress. Neither staging operation
changes the published checkpoint. Restart resets the cursor and processed count
while retaining the captured target/frontier, follow rule and rejections. Recording
a rejection never weakens an existing cascade and reports whether the hash was new.
Checked consumers compose planning/emission with staging and the token handshake
with a pending checkpoint. These are composition witnesses, not proofs of the
outer Materialization methods.

The production `Source` now maintains `processed + remaining == captured total`.
Its constructor validates the saved count and skips exactly that many canonical
records in bounded chunks. A checked consumer proves that replay and uninterrupted
emission leave exactly the same selection for the same History snapshot. Advancement
preserves captured metadata and visibility across growth, accounts for a pending
Reset event in its budget, advances the count without overflow and records the
correct completion cursor. Checkpoint preparation now derives progress and
completion fields from this maintained state. Wrong histories leave it unchanged;
an unwind during owned advancement leaves a source that rejects further use.

The actual record-to-event translator and cascade-rejection query are now checked.
New deliveries retain their stored author, ordered parents (including repeats) and
source payload ID, even when inactive. Activity requires captured after-visibility
and absence of direct or cascading rejection. Previously delivered records emit
exactly the permitted Activate/Retract transitions; rejected or unchanged records
emit none. Reset precedes the translated sequence. Counts, delivered-hash sets
and active-new-delivery lists describe that exact sequence.
A checked consumer composes real source advancement, translation, checkpoint
staging and batch issuance, proving that every activation in the issued batch is
inside the captured authorized set and passes the actual rejection query. This is
not yet a proof of the outer Materialization methods or application effects.

The production report-admission path now checks the actual batch token, rejection
membership/uniqueness and wanted-hash existence before consuming pending work.
All returned errors preserve the entire Protocol state. Stale-token errors take
precedence over report errors, and malformed rejections precede unknown-payload
errors. Success retains every rejection field and the exact deduplicated wanted
set, returns the original pending value and consumes the token once. Rejections
are checked against the pending source batch's delivery set, retained through
follow-up rounds; wanted hashes need only be known to the supplied History.
The actual rejection-recording helper merges cascade flags without weakening them,
counts exactly previously unrecorded hashes and preserves follow/cursor/progress.
Actual public `Applied::default` and `want` and pending construction are checked too.

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

Public `Materialization::step`/`update` now compose source, report and correction
transitions through Ledger/Document, including checked payload resolution,
checkpoint callback order and cumulative counters. Next verify local edits and
general lifecycle invariants, then connect a checked application to checkpoints.
Replay equivalence across reconstructed graphs or captures recompiled after growth
remains separate from the same-snapshot proof. `Checkpoint::rejected` iteration
and checkpoint codecs remain outside the harness.

`History::commits_since` now has a checked outer wrapper using the verified iterator. Commit preparation, admission/evidence, compaction,
sync minimality and attestation completion still need their own invariants and
proofs. They are not premises of the verified ancestry and difference queries.
