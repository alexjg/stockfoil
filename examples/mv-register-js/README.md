# Multi-value register in JavaScript

A small CRDT built on the Stockfoil Wasm binding (`../../javascript`): a map
from keys to multi-value registers. Concurrent writes to a key all survive until
a write that has seen them supersedes them.

- `codec.mjs`: an assignment's canonical payload is UTF-8 JSON `[key, value]`.
  The bundle codec writes each member as a u32 little-endian length followed by
  its canonical bytes; its decoder streams with bounded state.
- `register.mjs`: a `Replica` with
  - an Ed25519 key from `node:crypto` (`generateKeyPairSync('ed25519')`). The
    raw 32-byte public key is the Stockfoil author, and signatures are over the
    raw 32-byte commit hash (`prepared.signingBytes`);
  - `set(key, value)`: commit on the current heads, sign, and materialize;
  - `syncTo(peer)`: export what the peer has not acknowledged, `receive` each
    item there, and advance a per-peer `ExportCursor`;
  - `trust(author, 'unbounded' | 'denied')`: write permissions that decide
    which authors' commits participate;
  - materialization through `history.changes(cursor, view)` and
    `history.nextChanges(changes, max)`. Records with `newlyDelivered` are
    decoded and kept (even if they do not participate yet), and `visibility`
    (`after` / `before`) adds them to or removes them from the register. The
    `MaterializationCursor` is replaced only when a traversal is `complete`.

```js
import { Replica } from './register.mjs';

const alice = new Replica(), bob = new Replica();
await alice.trust(bob.author);
await bob.trust(alice.author);

await alice.set('color', 'green');
await bob.set('color', 'blue');
await alice.syncTo(bob);
await bob.syncTo(alice);
alice.get('color'); // ['blue', 'green']
```

## Run

Requires Node 22+, Rust with the `wasm32-unknown-unknown` target, and
`wasm-bodge`. `npm test` builds `../../javascript` first if it has not been built.

```sh
npm install
npm test
```
