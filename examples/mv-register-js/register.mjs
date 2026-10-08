// A multi-value register map replicated through Stockfoil.
//
// Each `set` is one signed commit whose parents are the replica's current heads.
// A key's value is the set of participating assignments to it that no other
// participating assignment to the same key descends from: concurrent writes
// all survive until a later write supersedes them.
import { generateKeyPairSync, sign } from 'node:crypto';
import { ExportCursor, Graph, MaterializationCursor, hex } from 'stockfoil';
import { decodeAssignment, encodeAssignment, registerCodec } from './codec.mjs';

export class Replica {
  graph = new Graph(registerCodec);
  /** Materialized state, advanced only through change batches. */
  cursor = new MaterializationCursor();
  assignments = new Map(); // hex hash -> { hash, key, value }, in delivery (parents-first) order
  participating = new Set(); // hex hashes
  values = new Map(); // key -> sorted values
  permissions = new Map(); // hex author -> [author, permission]
  exportCursors = new Map(); // peer -> ExportCursor
  #privateKey;
  #updating = Promise.resolve();

  constructor() {
    const { privateKey, publicKey } = generateKeyPairSync('ed25519');
    this.#privateKey = privateKey;
    // The raw 32-byte Ed25519 public key is the Stockfoil author.
    this.author = new Uint8Array(Buffer.from(publicKey.export({ format: 'jwk' }).x, 'base64url'));
    this.trust(this.author);
  }

  /** Let `author`'s writes participate (`'unbounded'`), or hide them (`'denied'`). */
  trust(author, permission = 'unbounded') {
    this.permissions.set(hex(author), [author, permission]);
    return this.update();
  }

  /** Write `value` to `key` on top of everything this replica has seen. */
  async set(key, value) {
    const parents = this.graph.history.heads('delivered');
    const prepared = await this.graph.commit(this.author, parents, encodeAssignment(key, value));
    // Signatures are over the raw 32-byte commit hash.
    const signature = new Uint8Array(sign(null, prepared.signingBytes, this.#privateKey));
    this.graph.sign(this.author, prepared.hash, signature);
    await this.update();
    return prepared.hash;
  }

  get(key) {
    return this.values.get(key) ?? [];
  }

  /** Send `peer` everything it has not acknowledged, then let it materialize. */
  async syncTo(peer) {
    const plan = this.graph.export(this.exportCursors.get(peer) ?? new ExportCursor());
    for (let i = 0; i < plan.length; i++) {
      await peer.graph.receive(await this.graph.writeItem(plan, i));
      plan.acknowledge(i);
    }
    this.exportCursors.set(peer, plan.cursor());
    await peer.update();
  }

  /** Apply change batches up to the current view. Updates run one at a time. */
  update() {
    this.#updating = this.#updating.catch(() => {}).then(() => this.#update());
    return this.#updating;
  }

  async #update() {
    const history = this.graph.history;
    const view = history.viewPoint(this.permissions.values());
    const changes = history.changes(this.cursor, view);
    const affected = new Set();
    for (;;) {
      const batch = history.nextChanges(changes, 64);
      for (const { hash, payload, newlyDelivered, visibility } of batch.records) {
        if (newlyDelivered) {
          // Keep every delivered assignment: a later view may enable it.
          const { key, value } = decodeAssignment(await this.graph.payload(hash));
          this.assignments.set(hex(hash), { hash, key, value });
        }
        const assignment = this.assignments.get(hex(hash));
        if (visibility === 'after') this.participating.add(hex(hash));
        if (visibility === 'before') this.participating.delete(hex(hash));
        if (visibility === 'after' || visibility === 'before') affected.add(assignment.key);
      }
      if (batch.complete) {
        this.cursor = batch.cursor;
        break;
      }
    }
    for (const key of affected) {
      const maximal = [];
      for (const a of this.assignments.values()) {
        if (a.key !== key || !this.participating.has(hex(a.hash))) continue;
        // Parents come first, so a superseding write always arrives after its ancestors.
        for (let i = maximal.length - 1; i >= 0; i--) {
          if (history.isAncestor(maximal[i].hash, a.hash)) maximal.splice(i, 1);
        }
        maximal.push(a);
      }
      this.values.set(key, maximal.map(a => a.value).sort());
    }
  }
}
