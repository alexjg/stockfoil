# Stockfoil wire format, version 1

## 1. Scope and conventions

This specification defines the byte representation and acceptance rules for a
Stockfoil **standalone commit** or **fragment**. It is intended to allow independent
implementations to exchange authenticated history. It does not require a particular
programming language, compression library, storage layout, or graph API.

The keywords **MUST**, **MUST NOT**, **SHOULD**, and **MAY** express requirements,
prohibitions, recommendations, and permitted choices, respectively. Hexadecimal
examples denote octets; whitespace in examples is not part of the encoding.
Indices are zero-based. Concatenation is written `||`. Integer arithmetic in this
specification is mathematical: overflow or truncation is never a valid result.

An item contains:

- Structure from which the members' ordered parent lists and authors are recovered.
- An application-defined payload body.
- Original-author signature evidence over reconstructed commit hashes.

The application format defines canonical payload bytes for each commit and a
lossless representation of a whole fragment's payloads. The surrounding Stockfoil
format is the same for all applications. Transport authentication does not replace
original-author evidence.

A transport supplies the boundary of each complete encoded item. This format has
no enclosing total-length field, transport identifier, permission map, encryption
layer, or format-negotiation exchange. Current library behavior, dependency
choices, resource defaults, and measurements are described separately in the
[implementation notes](codec-implementation.md).

## 2. Primitive encodings

### 2.1 Integers and byte strings

- `U32`: the shortest unsigned LEB128 encoding of a value in `0..2^32−1`.
- `U64`: the shortest unsigned LEB128 encoding of a value in `0..2^64−1`.
- `S64`: the shortest signed LEB128 encoding of a value in `−2^63..2^63−1`.
  This is signed LEB128, **not ZigZag followed by unsigned LEB128**.
- `Bytes`: a `U32` byte length followed by exactly that many octets.

Unsigned LEB128 emits the least-significant seven-bit groups first. Bit 7 is set
on every octet except the last. Signed LEB128 uses the same grouping, with the
last group's bit 6 determining sign extension. Its encoding ends as soon as the
remaining value is all sign bits and agrees with that bit. Nonminimal,
unterminated, truncated, and out-of-range encodings MUST be rejected.

Examples:

| Value | `U32` | `S64` |
| ---: | --- | --- |
| 0 | `00` | `00` |
| 64 | `40` | `c0 00` |
| 127 | `7f` | `ff 00` |
| 128 | `80 01` | `80 01` |
| −1 | not representable | `7f` |

For example, `80 00` is not a valid encoding of zero. Empty byte strings are
encoded as `00`; an empty *column body* contains zero octets.

### 2.2 Fixed-width values and ordering

Hashes and author identifiers occupy 32 octets each. An author identifier is the
encoded Ed25519 public key, with identity determined by its exact bytes.
Signatures occupy 64 octets. Fixed-width values have no length prefix.

Where sorting is required, compare raw byte sequences lexicographically, treating
each octet as unsigned. Do not interpret hashes as host-endian integers.

## 3. Column encodings

Each column is contained in one `Bytes` field. Its body MUST be consumed exactly,
and its expanded length MUST equal the cardinality required by the envelope.
There are no column-local padding bytes, block headers, or end markers.

### 3.1 Unsigned integer columns

Values are in `0..2^32−1`. A column consists of these runs:

| Run | Encoding | Expansion |
| --- | --- | --- |
| Repeat | `S64(c) || U32(v)`, with `2 ≤ c ≤ 2^63−1` | `c` copies of `v` |
| Literal | `S64(−c) || U32(v₀) || … || U32(v[c−1])`, with `1 ≤ c ≤ 2^63−1` | The listed values |

Headers zero and `−2^63` are forbidden. There are no null values or null runs.
A single value MUST be encoded as a one-value literal, not a repeat run.

The following canonicality rules also apply:

1. Consecutive values within a literal run MUST differ.
2. Two literal runs MUST NOT be adjacent.
3. Two adjacent repeat runs MUST have different values.
4. At a repeat/literal boundary, the repeat value MUST differ from the first
   literal value. At a literal/repeat boundary, the last literal value MUST
   differ from the repeat value.

Values may recur after intervening different values. Adjacent repeat runs with
different values are permitted; run types need not alternate.

Examples below are column bodies, without the enclosing `Bytes` length:

| Expanded values or condition | Octets | Valid? |
| --- | --- | --- |
| `[]` | empty | yes |
| `[1]` | `7f 01` | yes |
| `[0, 0]` | `02 00` | yes |
| `[1, 3]` | `7e 01 03` | yes |
| `[1, 2, 1]` | `7d 01 02 01` | yes |
| `[1, 1, 2, 2]` | `02 01 02 02` | yes |
| Singleton repeat | `01 01` | no |
| Equal adjacent literal values | `7e 01 01` | no |
| Adjacent literal runs | `7f 01 7f 02` | no |
| Mergeable repeat runs | `02 01 02 01` | no |

### 3.2 Boolean columns

A boolean column is a sequence of `U64` run lengths. Runs alternate **false, true,
false, true, …**, starting with false. All lengths MUST be positive, except that
an initial zero-length false run MAY precede a positive true run. An initial zero
alone, any interior zero, and any trailing zero MUST be rejected.

| Expanded values | Column body |
| --- | --- |
| `[]` | empty |
| `[false, false]` | `02` |
| `[true, true]` | `00 02` |
| `[false, true, false, true]` | `01 01 01 01` |

Run lengths are not limited to the `U32` value domain of integer columns. All
runs still have to fit the column's required expanded length.

## 4. Envelope layout

Both item kinds use the following fields, in exactly this order, without padding
or alignment. `IntegerColumn(K)` and `BooleanColumn(K)` each mean a `Bytes` field
using section 3's encoding, expanding to exactly `K` values.

| Field | Encoding and constraints |
| --- | --- |
| Magic | `53 54 4b 46` (`STKF`) |
| Version | `01`; other versions are not v1 |
| Kind | `00` for standalone commit; `01` for fragment; other values invalid |
| Application format ID | `Bytes`; 1–128 octets of well-formed UTF-8 |
| Claimed head | 32 octets: standalone commit hash or fragment head hash |
| Boundary count `B` | `U32`; MUST be zero for standalone commits |
| Boundaries | `B` hashes, strictly increasing |
| Member count `N` | `U32`; positive; MUST be one for standalone commits |
| Author count `A` | `U32` |
| Author dictionary | `A` author identifiers, strictly increasing |
| External-parent count `E` | `U32` |
| External-parent dictionary | `E` hashes, strictly increasing |
| Member authors | `IntegerColumn(N)`, indices into the author dictionary |
| Member parent counts | `IntegerColumn(N)`; let their sum be `P` |
| Parent kinds | `BooleanColumn(P)`; let the number of true values be `I` |
| Internal parent distances | `IntegerColumn(I)` |
| External parent indices | `IntegerColumn(P−I)`, indices into the external-parent dictionary |
| Signature count `S` | `U32` |
| Signature signers | `IntegerColumn(S)`, indices into the author dictionary |
| Signature targets | `IntegerColumn(S)`, indices into the member sequence |
| Signatures | `S` consecutive 64-octet signatures, aligned with the preceding two columns |
| Application payload body | `Bytes` |

Truncated fields and trailing octets MUST be rejected. The sum `P` is not itself
a transmitted `U32` and MUST NOT be computed with wrapping arithmetic.

Every used dictionary index and signature target MUST be in range. Boundaries
and dictionaries MUST NOT contain duplicate entries. Unused dictionary entries
are permitted. Signature rows need not be sorted or unique: duplicates and
additional evidence are permitted, but **every row MUST verify**.

Checkpoint prefixes are not transmitted. A transport may carry derived
reconciliation hints separately, but they are not signature evidence and MUST NOT
replace any check in this specification.

## 5. Application format and member reconstruction

### 5.1 Application format agreement

A receiving context MUST select its application format through trusted
configuration and reject an incoming format ID that does not match exactly.
Comparison is of the UTF-8 bytes, without case folding or Unicode normalization.
The received ID is not permission to execute or fetch a codec supplied by a peer.

The application format MUST specify:

- Which standalone payload bytes are valid and canonical.
- How a fragment body and its logical member shape yield exactly `N` canonical
  commit payload byte strings, one per member in the supplied order.

Fragment decoding MUST NOT require payloads from omitted dependency history.
The logical shape consists of authors and ordered internal/external parent
references as defined below. A body or reconstructed payload that violates the
application format MUST be rejected. For standalone items the body itself MUST
be the canonical payload bytes; accepting a noncanonical spelling and silently
normalizing it is not permitted.

Application format IDs and the envelope version are separate. The format ID is
**not included in the commit hash**. Applications requiring cryptographic domain
separation MUST provide it in their canonical payload representation.

### 5.2 Parent reconstruction

Members are ordered ancestors before descendants. Flatten parent references in
member order, consuming each member's declared parent count. For each reference:

- If the next parent-kind value is true, consume an internal distance `d`. For
  current member index `i`, require `1 ≤ d ≤ i`; the parent is member `i−d`.
- Otherwise consume an external-parent dictionary index; the parent is that hash.

Parent order and repeated references MUST be preserved. They affect identity,
even though ancestry queries treat repeated edges as a single relationship.
All three flattened parent columns MUST be consumed exactly.

After reconstructing member hashes, a referenced external hash MUST NOT equal
**any** member hash. It is not an alternative spelling for an internal reference.
An unused external dictionary entry is not subject to this exclusion.

### 5.3 Commit identity

Let `payloadᵢ` be member `i`'s canonical application bytes, `authorᵢ` its 32-byte
author identifier, and `parentsᵢ` its ordered reconstructed parent hashes.
Compute members in sequence:

```text
payload_hashᵢ = BLAKE3(payloadᵢ)
preimageᵢ = J(length(parentsᵢ)) || parentsᵢ[0] || … || parentsᵢ[last]
            || authorᵢ || payload_hashᵢ
hashᵢ = BLAKE3(preimageᵢ)
```

BLAKE3 is used in its ordinary unkeyed mode with 32-octet output, without a context
string or additional prefix. Hashes in the preimage are raw bytes, not hexadecimal
text. For a parentless commit, the parent-hash sequence is empty.

**`J` is not LEB128.** For the parent-count domain permitted by v1, it is defined
completely by this table. `BEₖ(x)` is exactly `k` big-endian octets, including any
leading zero octets:

| Parent count `n` | `J(n)` |
| --- | --- |
| `0 ≤ n ≤ 247` | One octet containing `n` |
| `248 ≤ n ≤ 503` | `f8 || BE₁(n−248)` |
| `504 ≤ n ≤ 66,039` | `f9 || BE₂(n−504)` |
| `66,040 ≤ n ≤ 16,843,255` | `fa || BE₃(n−66,040)` |
| `16,843,256 ≤ n ≤ 2^32−1` | `fb || BE₄(n−16,843,256)` |

Examples: `J(128) = 80`, `J(248) = f8 00`, `J(504) = f9 00 00`, and
`J(2^32−1) = fb fe fe fe 07`. This identity encoding is distinct from both
outer framing and column encodings.

Duplicate reconstructed member hashes MUST be rejected. For a standalone item,
the sole member hash MUST equal the claimed head. For a fragment, the claimed
head MUST be a reconstructed member and satisfy section 7.

Signatures, boundary declarations, format IDs, column packing, and dictionary
layout do not occur in the commit-hash preimage. Changing canonical payload
bytes changes identity; changing only a valid fragment representation does not.

## 6. Authentication

Each signature row identifies a signer and a target member. Its message is exactly
the target member's **32 raw hash bytes**. It does not sign the envelope, the hash
preimage, or hexadecimal text. Signatures MUST satisfy the verification profile
in appendix A. One invalid row invalidates the item, even if other rows would
provide sufficient coverage without it.

For each original author `a` occurring in member metadata, let:

- `Mₐ` be the set of supplied members authored by `a`.
- `Tₐ` be the target members of valid signature rows whose signer bytes equal `a`.
- `Ancₘ(Tₐ)` be the reflexive ancestry of those targets, following parent edges
  **only within supplied members**.

The item is authenticated only if, for every such author:

```text
Mₐ ⊆ Ancₘ(Tₐ)
```

Ancestry includes the target itself and may cross commits by other authors. It
stops at external parents: signatures elsewhere in dependency history cannot
supply missing evidence for this item.

The signer need not be the target's original author. For a chain authored by
A, then B, then A, A's signature on the last commit covers both A commits. B still
needs B-signed evidence on the middle commit or on a supplied descendant. Concurrent
authored tips all need coverage; a signature on their supplied common descendant
can cover them together.

A standalone item therefore needs its original author's signature on that commit.
Additional valid signatures do not grant write permission, and one author's
signature never substitutes for another author's coverage.

## 7. Fragment membership and dependency history

For a hash `h`, define `depth(h)` as the number of consecutive zero **octets** at
the beginning of its raw 32 bytes. This is not a count of zero bits.

Let `h` be the claimed head, `D = depth(h)`, `M` the reconstructed member set,
`Bnd` the declared boundary set, and `Ext` the distinct external parents actually
referenced by members (not all dictionary entries).

### 7.1 Checks independent of dependency history

A fragment MUST satisfy all of the following:

1. `D > 0` and `h ∈ M`.
2. Every boundary hash has depth at least `D`.
3. No boundary hash belongs to `M`.
4. Every member other than `h` has depth less than `D`.
5. Every external parent not in `Bnd` has depth less than `D`.
6. Every member is reachable from `h` by following parent edges within `M`.
7. Member references are acyclic and the authentication rules of section 6 hold.

Standalone items are not subject to these depth or fragment-placement rules:
a standalone commit may have any depth.

### 7.2 Checks requiring authenticated history

A standalone item's dependencies are its distinct parent hashes. A fragment's
dependencies are `Ext ∪ Bnd`. Declared boundaries and external parents are
**different sets**: a low-depth external parent may lie in excluded history
without being a boundary itself.

Before admitting a fragment, a receiver MUST have authenticated, parent-closed
history for all its dependencies. Let `X` be the boundaries and all their ancestors
in that history. The receiver MUST establish:

1. `M ∩ X` is empty: no supplied member belongs to excluded boundary history.
2. `Ext ⊆ X`: all references leaving the fragment connect to excluded history.
3. Starting at every hash in `Ext`, walk parent edges in authenticated history.
   Stop each path at the first node whose depth is at least `D`, and collect that
   node. The set of all collected stops MUST equal `Bnd` exactly. A parentless
   node below that depth contributes no stop.

In particular, stopping only when a *listed* boundary is encountered is
insufficient; the threshold is depth at least `D`. Boundaries need not form an
ancestry antichain, and MUST NOT be reduced to one as a normalization step.

These conditions establish the intended membership:

```text
M = inclusive_history(h) − union(inclusive_history(b) for b in Bnd)
```

Missing history is not, by itself, evidence of malformed encoding or invalid
signatures. A receiver MAY defer the item until dependencies are available, but
MUST NOT expose its members as admitted history or use them as authenticated
dependencies before the required checks succeed. Local authorization of admitted
history for application use is a separate policy.

## 8. Conformance and resource policy

A conforming encoder MUST produce items satisfying the syntax, application-format,
authentication, and membership rules above. A receiver MUST reject violations;
successful parsing alone does not establish authentication or admission.

Receivers MAY impose local limits on item bytes, members, parent references,
signatures, reconstructed payload size, pending history, or computational work.
Such refusal is a resource-policy outcome, not a different wire representation.
There are no protocol-wide 8 MiB or 65,536-member limits. Implementations MUST
NOT wrap, truncate, or partially accept a count to fit local arithmetic or budgets.
They SHOULD enforce limits before expanding compressed runs or allocating from
untrusted counts. Small encoded bodies can describe large decoded results.

Version 1 defines neither capability negotiation for these limits nor authenticated
fragment chunking. A sender's valid item may exceed a receiver's configured
capacity; the surrounding application must decide whether to retry with greater
capacity or decline it.

Canonical columns do **not** imply a unique byte representation of an item.
Receivers MUST NOT require a particular topological order beyond section 5, a
minimal dictionary, a particular sufficient signature set/order, or an identical
application fragment packing when the application format permits alternatives.
Item identity is the commit hash for a standalone item, or the pair `(head,
boundary set)` for a fragment; it is not a hash of the envelope bytes.

## 9. Interoperability vectors

The linked files contain complete v1 envelopes as hexadecimal text. These are
valid example representations, not a requirement that all equivalent exports
produce the same bytes.

Both vectors use application format ID `test/runs/v1`:

- Canonical commit payloads are well-formed UTF-8 string bytes, without a length
  prefix or Unicode normalization.
- A fragment body is a sequence of records: a positive repetition count as a
  **fixed-width little-endian u32**, a string byte length in the same encoding,
  and that many UTF-8 octets. Each record produces that many identical payloads.
  Records continue to the end of the body and MUST expand to exactly `N` payloads.
- This application body format is independent of the envelope's LEB128 columns.

The author/public key in both vectors is:

```text
ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c
```

It is derived from the Ed25519 seed consisting of 32 repetitions of octet `07`.

### 9.1 Standalone commit

[Complete envelope: `codec-commit-v1.hex`](../tests/fixtures/codec-commit-v1.hex)
(173 octets).

- One parentless member with canonical payload `hello` (`68 65 6c 6c 6f`).
- Empty boundary and external-parent dictionary.
- One signature by the author, targeting member 0.
- Reconstructed hash and claimed head:

```text
ea2a999164d3363faffdc4263955bcb022710eb844362f87c46eccb5bf915436
```

### 9.2 Fragment

[Complete envelope: `codec-fragment-v1.hex`](../tests/fixtures/codec-fragment-v1.hex)
(256 octets).

- 101 members by the same author, in a chain. Member 0 is parentless; every later
  member has exactly the preceding member as its parent.
- Members 0–99 each have canonical payload `shared long application key and repeated value`
  (46 UTF-8 octets). Member 100 has `checkpoint:129955` (17 octets).
- The body has two records: counts 100 and 1, lengths 46 and 17, respectively.
- Empty boundary and external-parent dictionary; one signature targeting member 100.
- The reconstructed head has depth 2:

```text
0000bf36ab272ffdc22d98460eae2c0ab59764a0a5e28ac9ad9a7d791e5ae0ae
```

Together with the scalar and column examples above, these vectors distinguish
payload packing, structure reconstruction, identity hashing, and signature
verification. An independent implementation should validate both without access
to omitted dependency history.

## Appendix A. Signature verification profile (normative)

This appendix fixes the acceptance rules; a library function named “strict
Ed25519” is not by itself a sufficient definition.

Use the Edwards25519 curve `−x² + y² = 1 + d·x²·y²` over the field
`p = 2^255−19`, where `d = −121665/121666 mod p`. Let `G` be the standard Ed25519
base point (its `y` coordinate is `4/5 mod p`, and its `x` coordinate is even),
`O` the identity, and
`ℓ = 2^252 + 27742317777372353535851937790883648493`.

Point decoding interprets a 32-octet string as a little-endian integer: bit 255
is the `x` sign/parity bit and the lower 255 bits give `y_raw`. Set `y = y_raw mod p`
and recover `x` satisfying the curve equation. Decoding fails if no such point
exists. For nonzero `x`, select the square root whose canonical field
representative has the supplied parity. For `x = 0`, either sign bit decodes to
zero. Canonical point encoding uses `y` in `0..p−1`, with the parity of canonical
`x` in bit 255 (zero when `x = 0`).

For signer bytes `A_bytes`, signature `R_bytes || S_bytes` (32 octets each), and
the target's 32-octet commit hash `m`, verification MUST enforce all of:

1. Decode `A_bytes` and `R_bytes` to points `A` and `R` as above.
2. Interpret `S_bytes` as a little-endian integer `s`; require `0 ≤ s < ℓ`.
   Do not reduce a noncanonical `s` modulo `ℓ` and accept it.
3. Reject small-order points: require `[8]A ≠ O` and `[8]R ≠ O`.
4. Compute `k = LE_integer(SHA-512(R_bytes || A_bytes || m)) mod ℓ`, using all
   64 digest octets. The challenge uses the **original supplied key and R bytes**,
   not normalized point encodings.
5. Require the canonical encoding of `[s]G − [k]A` to equal `R_bytes` exactly.

This is not Ed25519ph and has no context string. The signature's message is the
commit hash, not a further SHA-512 prehash supplied as the message.

These rules intentionally distinguish key and signature encodings: `S` must be
canonical, and the final equality forces canonical `R` bytes. There is no analogous
canonical-byte requirement for `A`. Noncanonical key encodings are subject to the
same equation using their original bytes, and denote distinct Stockfoil authors.
Unused author-dictionary entries need not decode as curve points.

Small-order rejection is not a prime-subgroup requirement: points with both
nonzero prime-order and torsion components are not rejected merely for being
mixed-order. Implementations MUST use the equation above, with `k` reduced modulo
`ℓ`; substituting a cofactored equation or an additional prime-subgroup check
changes the acceptance rules.
