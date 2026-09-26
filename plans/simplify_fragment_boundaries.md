# Simplify fragment boundaries and dependencies

## Motivation

Fragment boundaries and payload dependencies describe different things. A fragment's boundary is discovered by walking backwards from its head and stopping at commits whose depth is at least the fragment's level. Its members, however, are the head's inclusive history minus the inclusive histories of all boundary commits:

```text
members = H(head) - union(H(boundary))
```

Consequently, a member can reference a parent that is excluded through boundary history without that parent itself being a boundary. Conversely, a boundary can be discovered only through excluded history and need not be directly referenced by any member.

The current `dependencies`, `additional_dependencies`, and `direct_boundary` fields encode these distinctions, but duplicate information and make the representation harder to understand than necessary.

This plan is a **representation/API refactor, leaving fragment semantics unchanged**. In particular, keep `fragment_boundary()` and `fragment_members()` as they are. The simplification is to **store the range and the actual external parents, and derive everything else**.

## 1. Remove dependency declarations from transport items

In `src/ingest.rs`, change the fragment variant to:

```rust
Fragment {
    range: FragmentRange,
    commits: Vec<CommitMeta>,
    signatures: Vec<SignatureEvidence>,
}
```

Likewise, `verify_fragment()` becomes:

```rust
pub fn verify_fragment(
    range: FragmentRange,
    signatures: Vec<SignatureEvidence>,
    commits: impl IntoIterator<Item = Result<CommitMeta, VerificationError>>,
) -> Result<VerifiedItem, VerificationError>
```

There is no reason for the sender to declare something we can calculate exactly from the metadata it supplies:

```text
external_parents = parents(members) - members
```

This calculation must happen after collecting the member hashes: an earlier parent reference might refer to a member arriving later in the input.

The existing verifier already collects everything into a map, so this does not introduce a new buffering requirement.

## 2. Give `VerifiedItem` one dependency-related field

Replace:

```rust
dependencies: BTreeSet<CommitHash>,
additional_dependencies: BTreeSet<CommitHash>,
direct_boundary: BTreeSet<CommitHash>,
```

with:

```rust
external_parents: BTreeSet<CommitHash>,
```

The range boundary already lives in `ItemId::Fragment(range)`, so we do not need another copy.

Expose:

```rust
/// Exact parent references leaving the supplied members.
pub fn external_parents(&self) -> impl Iterator<Item = &CommitHash>;

/// All prerequisites required before admission.
/// Fragment boundary ∪ external parents; standalone parents otherwise.
pub fn dependencies(&self) -> impl Iterator<Item = &CommitHash>;
```

Retain `dependencies()` for scheduling compatibility, documenting it as **admission prerequisites**, not “the fragment boundary.” It should remain sorted and deduplicated. Remove the `additional_dependencies()` accessor in favor of `external_parents()`.

For standalone commits, external parents are simply the commit's parents.

## 3. Keep the important stateless checks

`validate_fragment()` should check the computed external parents rather than a separately declared set.

Keep:

- Head membership and depth.
- Boundary depth.
- Exclusion of boundaries from members.
- All other members having depth below the head.
- Every member being reachable from the head.
- Acyclicity and signature coverage.
- **Every external parent at boundary depth or deeper must appear in the declared boundary.**

Low-depth external parents are allowed provisionally. Admission must still prove that they belong to boundary history.

Removing the explicit declaration does **not** mean accepting arbitrary omissions. Omitting an in-range parent may pass stateless verification, but must fail admission—just as declaring it an extra does today.

## 4. Simplify admission to one external-parent walk

`src/commit_graph.rs::validate_fragment_admission()` no longer needs to distinguish “direct boundary” from “additional dependency.”

Conceptually:

```text
Require every dependency to be admitted.

excluded = history(range.boundary)

Reject any member in excluded.
Reject any external parent outside excluded.

Walk backwards from all external parents:
    stop at commits of depth >= head depth
    collect those stopping points

Require collected stopping points == range.boundary.
```

Starting from **all external parents** naturally handles both cases:

- A directly referenced boundary stops immediately.
- A low-depth external parent is followed through excluded history to discover further boundaries.

This preserves the current optimization: **we still never rewalk supplied member edges during admission**. Their reachability was established during verification; the external-parent set records exactly where those edges leave the item.

It also eliminates `direct_boundary` entirely rather than merely renaming it.

## 5. Remove producer-side bookkeeping

In `src/commit_graph.rs`:

- Delete `fragment_additional_dependencies()`.
- Remove its calls from `export()` and `local_item()`.
- Leave boundary discovery, normalized member enumeration, and certificate selection unchanged.

`UnverifiedItem::dependencies()` can compute external parents from its in-memory metadata and union them with the boundary. These remain untrusted prerequisites until verification/admission.

For `local_item()`, verify first and check prerequisites on the resulting `VerifiedItem`, avoiding computing external parents twice.

## 6. Update tests around the distinction

Most constructor changes are mechanical. The substantive tests should establish:

1. **Ordinary fragment:** external parents equal the range boundary.
2. **Bypass:** an external parent lies below the boundary.
3. **Indirect boundary:** a range boundary is not itself an external parent.
4. **Omitted in-range parent:** stateless verification can succeed, but admission rejects it—even if that parent is already admitted.
5. **Included boundary ancestor:** admission rejects non-normalized contents.
6. Dependencies remain sorted and deduplicated; streaming and owned verification agree.
7. Rejection still occurs before payload callbacks or mutation.

Tests for missing or spurious *declared extras* disappear because that declaration no longer exists. Their replacement is testing exact derivation from member metadata.

Update `design/sedimentree_sync.md` to use the two concepts consistently: **range boundary** and **external parents**.

## Outcome

The result is the same security model and fragment contents, but much less duplicated state: the range describes **which history is excluded**, and external parents describe **where the supplied metadata connects to that history**.
