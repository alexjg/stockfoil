import { generateKeyPairSync, sign } from 'node:crypto';
import { CommitGraph, MaterializationCursor, authorId } from 'stockfoil';
import { encodeAssignment, decodeAssignment } from './wire.mjs';

export function checkReport(report) {
  if (report.rejected.length) throw new AggregateError(report.rejected.map(r => r.error), 'queued history rejected');
}

class Materialized {
  cursor = new MaterializationCursor();
  assignments = [];
  participating = new Set();
  values = new Map();
  clone() {
    const copy = new Materialized();
    copy.cursor = this.cursor.clone();
    copy.assignments = this.assignments.slice();
    copy.participating = new Set(this.participating);
    copy.values = new Map([...this.values].map(([k, v]) => [k, new Set(v)]));
    return copy;
  }
  update(graph, view) {
    const batch = graph.changes(this.cursor, view);
    const affected = new Set();
    for (const change of batch.changes) {
      const { hash, payload } = change.commit;
      const assignment = decodeAssignment(payload);
      if (change.newlyDelivered) this.assignments.push([hash, assignment]);
      if (change.visibility === 'after') {
        this.participating.add(hash); affected.add(assignment.key);
      } else if (change.visibility === 'before') {
        this.participating.delete(hash); affected.add(assignment.key);
      }
    }
    for (const key of affected) {
      const maximal = new Map();
      // Retained writes are in ancestors-before-descendants delivery order.
      // Revocation can reveal an older assignment not present in this batch.
      for (const [hash, assignment] of this.assignments) {
        if (assignment.key !== key || !this.participating.has(hash)) continue;
        for (const older of maximal.keys()) {
          if (graph.isAncestor(older, hash)) maximal.delete(older);
        }
        maximal.set(hash, assignment.value);
      }
      this.values.set(key, new Set(maximal.values()));
    }
    this.cursor = batch.cursor;
  }
  get(key) { return [...(this.values.get(key) ?? [])].sort(); }
}

export class MvRegisterMap {
  graph = new CommitGraph();
  materialized = new Materialized();
  pending = new Map();
  constructor() {
    const { privateKey, publicKey } = generateKeyPairSync('ed25519');
    this.privateKey = privateKey;
    this.author = authorId(Buffer.from(publicKey.export({ format: 'jwk' }).x, 'base64url'));
    this.setWritePermissions(new Map([[this.author, 'unbounded']]));
  }
  update() { this.materialized.update(this.graph, this.graph.viewPoint()); }
  setWritePermissions(permissions) { this.graph.setWritePermissions(permissions); this.update(); }
  viewPoint() { return this.graph.viewPoint(); }
  get(key) { return this.materialized.get(key); }
  getAt(view, key) {
    const historical = this.materialized.clone();
    historical.update(this.graph, view);
    return historical.get(key);
  }
  prepareSet(key, value) {
    const draft = this.graph.commit(this.author, encodeAssignment({ key, value }));
    checkReport(draft.report);
    this.pending.set(draft.hash, draft.signingBytes);
    this.update();
    return draft.hash;
  }
  publish() {
    for (const [hash, signingBytes] of this.pending) {
      if (!this.graph.hasCommit(hash)) {
        // Node owns the key and signs externally; Wasm verifies every signature.
        checkReport(this.graph.addSignature({
          author: this.author, commit: hash,
          signature: new Uint8Array(sign(null, signingBytes, this.privateKey)),
        }));
      }
      this.pending.delete(hash);
    }
    this.update();
  }
  set(key, value) { const hash = this.prepareSet(key, value); this.publish(); return hash; }
  ingest(items) {
    try {
      for (const item of items) {
        // Check application syntax before mutating the graph. This establishes
        // no trust: only ingest can authenticate the bytes and admit history.
        for (const commit of item.kind === 'commit' ? [item.commit] : item.commits) decodeAssignment(commit.payload);
        checkReport(this.graph.ingest(item));
      }
    } finally { this.update(); }
    return this.graph.pendingItems();
  }
  dispose() { this.graph.dispose(); }
}
