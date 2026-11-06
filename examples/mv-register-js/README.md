# Multi-Value Register Map (JavaScript)

A runnable multi-value register CRDT built on Stockfoil's `Document` and
`Materialization` in JavaScript.

When two peers write to the same key concurrently, both values are preserved
until a causally later commit supersedes them. If a winning write is later
revoked via a write frontier, the register automatically falls back to the
earlier concurrent or ancestor value.

- **[`register.mjs`](register.mjs):** Wraps a `Document` and a `Materialization`
  (`supportsVisibilityChanges: true`). Consumes `"deliver"`, `"activate"`,
  `"retract"`, and `"reset"` events in batches, retaining inactive assignments
  so they can be activated later or referenced by descendants, and uses
  `ledger.isAncestor(a, b)` to compute the causally maximal active values for
  each key.
- **[`codec.mjs`](codec.mjs):** Encodes each commit's canonical payload as UTF-8
  JSON `[key, value]`, and implements a length-prefixed bundle `Codec` with a
  bounded streaming decoder.

```js
import { Replica } from "./register.mjs";

const alice = new Replica();
const bob = new Replica();
alice.trust(bob.author);
bob.trust(alice.author);

alice.set("color", "green");
bob.set("color", "blue");
alice.syncTo(bob);
bob.syncTo(alice);

alice.get("color"); // ["blue", "green"]

// A subsequent write supersedes the concurrent values:
const c = alice.set("color", "purple");
alice.syncTo(bob);
bob.get("color"); // ["purple"]
```

## Running the Example and Tests

Requires Node 22+ and the [JavaScript binding build tools](../../javascript).
If you modify `javascript/src`, rebuild the binding before running the example's
tests:

```sh
npm --prefix ../../javascript install
npm --prefix ../../javascript run build
npm install
npm test
```
