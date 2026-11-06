// Multi-value register: application state consumes Rust planner batches.
// Transport/repository orchestration stays outside Stockfoil.
import { generateKeyPairSync, sign } from 'node:crypto';
import { acknowledgeExport, Document, Materialization, hex } from 'stockfoil';
import { decodeAssignment, encodeAssignment, registerCodec } from './codec.mjs';

export class Replica {
  document;
  materialization = new Materialization({ supportsVisibilityChanges: true });
  assignments = new Map(); // includes invisible assignments needed by later views
  participating = new Set();
  values = new Map();
  exportCursors = new Map();
  #privateKey;

  constructor() {
    const { privateKey, publicKey } = generateKeyPairSync('ed25519');
    this.#privateKey = privateKey;
    this.author = new Uint8Array(Buffer.from(publicKey.export({ format: 'jwk' }).x, 'base64url'));
    this.document = new Document(registerCodec, { author: this.author });
    this.update();
  }

  trust(author, permission = 'unbounded') {
    const ledger = this.document.ledger;
    const permissions = ledger.permissions().filter(([a]) => hex(a) !== hex(author));
    permissions.push([author, permission]);
    ledger.setPermissions(permissions);
    this.update();
  }

  /** Local edits, signing and materialization are synchronous. */
  set(key, value) {
    const draft = this.document.commit(encodeAssignment(key, value));
    this.document.attachSignature(draft, new Uint8Array(sign(null, draft.hash, this.#privateKey)));
    this.update();
    return draft.hash;
  }

  get(key) { return this.values.get(key) ?? []; }

  syncTo(peer) {
    const plan = this.document.export(this.exportCursors.get(peer) ?? []);
    plan.items.forEach((item, i) => {
      peer.document.receive(this.document.exportItem(item));
      acknowledgeExport(plan, i);
    });
    this.exportCursors.set(peer, plan.cursor);
    peer.update();
  }

  update() {
    const ledger = this.document.ledger;
    while (!this.materialization.isCurrent(ledger)) {
      let batch = this.materialization.plan(ledger, 64);
      while (batch) {
        const applied = this.#apply(batch.events);
        batch = this.materialization.complete(ledger, batch, applied);
      }
    }
    // A durable application would save assignments/participation and
    // materialization.checkpoint together here, after all follow-up rounds.
  }

  #apply(events) {
    const rejected = [];
    for (const event of events) {
      if (event.kind === 'reset') {
        this.assignments.clear();
        this.participating.clear();
        continue;
      }
      const id = hex(event.hash);
      if (event.kind === 'deliver') {
        try {
          const { key, value } = decodeAssignment(this.document.payloadById(event.payload));
          this.assignments.set(id, { hash: event.hash, key, value });
          if (event.active) this.participating.add(id);
        } catch (error) {
          rejected.push({ hash: event.hash, reason: error.message });
        }
      } else if (event.kind === 'activate') this.participating.add(id);
      else if (event.kind === 'retract') this.participating.delete(id);
    }
    this.#recompute();
    return { rejected };
  }

  #recompute() {
    this.values.clear();
    const byKey = new Map();
    const ledger = this.document.ledger;
    for (const [id, a] of this.assignments) {
      if (!this.participating.has(id)) continue;
      const maximal = byKey.get(a.key) ?? [];
      // Compare against the current frontier, not every historical assignment.
      // Checking both directions also works independently of insertion order.
      if (maximal.some(b => ledger.isAncestor(a.hash, b.hash))) continue;
      for (let i = maximal.length - 1; i >= 0; i--) {
        if (ledger.isAncestor(maximal[i].hash, a.hash)) maximal.splice(i, 1);
      }
      maximal.push(a);
      byKey.set(a.key, maximal);
    }
    for (const [key, maximal] of byKey) {
      this.values.set(key, maximal.map(a => a.value).sort());
    }
  }
}
