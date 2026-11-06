# Multi-value register in JavaScript

A runnable map of multi-value registers built on Stockfoil's `Document` and
`Materialization`. Concurrent assignments survive until a later assignment
supersedes them.

- `codec.mjs`: canonical UTF-8 JSON `[key, value]`; bundle members use u32
  little-endian length framing and a bounded streaming decoder.
- `register.mjs`: synchronous local commits and Ed25519 signing over raw commit
  hashes. The ledger owns document-level write permissions and parking.
  Independent materialization progress is handled by the Rust planner.
- The register consumes plain `deliver`, `activate`, `retract` and `reset`
  events in batches. It retains invisible assignments for later activation and
  reports malformed payloads as permanent per-materialization rejections.
- `syncTo` moves captured export descriptions and wire bytes, then acknowledges
  them using plain export cursors. It is example transport code, not a required
  Stockfoil protocol.
- Durable applications must persist register state and
  `materialization.checkpoint` atomically after all follow-up batches complete.
  This example keeps state in memory.

```js
import { Replica } from './register.mjs';

const alice = new Replica(), bob = new Replica();
alice.trust(bob.author);
bob.trust(alice.author);

alice.set('color', 'green');
bob.set('color', 'blue');
alice.syncTo(bob);
bob.syncTo(alice);
alice.get('color'); // ['blue', 'green']
```

## Run

Requires Node 22+ and the [JavaScript binding's build tools](../../javascript).
Build the binding before testing changes to its source:

```sh
npm --prefix ../../javascript install
npm --prefix ../../javascript run build
npm install
npm test
```
