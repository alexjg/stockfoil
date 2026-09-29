import assert from 'node:assert/strict';
import { createPrivateKey, createPublicKey, sign } from 'node:crypto';
import {
  authorId, CommitGraph, describeCommit, ExportCursor, MaterializationCursor,
  StockfoilError,
} from 'stockfoil';

export const bytes = (text) => new TextEncoder().encode(text);
export const copy = (value) => structuredClone(value);

// RFC 8410 PKCS#8 wrapping a deterministic Ed25519 seed. Signing stays in Node;
// describeCommit delegates payload/metadata hashing to the native implementation.
export function signer(seed) {
  const key = createPrivateKey({
    key: Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), Buffer.alloc(32, seed)]),
    format: 'der', type: 'pkcs8',
  });
  const publicKey = new Uint8Array(createPublicKey(key).export({ format: 'der', type: 'spki' }).subarray(-32));
  const author = authorId(publicKey);
  const signBytes = (input) => new Uint8Array(sign(null, input, key));
  return {
    author, publicKey, signBytes,
    proof(hash) {
      return { author, commit: hash, signature: signBytes(Buffer.from(hash, 'hex')) };
    },
  };
}

export const alice = signer(1);
export const bob = signer(2);
export const carol = signer(3);
export const unknown = 'ff'.repeat(32);

const mined = new Map();
export function atDepth(who, parents = [], depth = 0) {
  const cacheKey = JSON.stringify([who.author, parents, depth]);
  if (!mined.has(cacheKey)) {
    for (let nonce = 0; nonce < 10_000_000; nonce++) {
      const payload = new Uint8Array(8);
      new DataView(payload.buffer).setBigUint64(0, BigInt(nonce), true);
      const commit = describeCommit(who.author, parents, payload);
      if (commit.hash.startsWith('00'.repeat(depth)) && commit.hash.slice(depth * 2, depth * 2 + 2) !== '00') {
        mined.set(cacheKey, commit);
        break;
      }
    }
  }
  assert.ok(mined.has(cacheKey), `mine depth ${depth}`);
  return copy(mined.get(cacheKey));
}

export function signedCommit(commit, who = alice) {
  return { kind: 'commit', commit: copy(commit), signatures: [who.proof(commit.hash)] };
}

// Do not normalize malformed fragments: tests must exercise native validation.
export function fragment(head, commits, boundary = [], signatures = [alice.proof(head.hash)]) {
  return { kind: 'fragment', head: head.hash, boundary: [...boundary], commits: copy(commits), signatures };
}

export function itemId(item) {
  return item.kind === 'commit'
    ? { kind: 'commit', hash: item.commit.hash }
    : { kind: 'fragment', head: item.head, boundary: [...item.boundary].sort() };
}

export function assertIds(actual, expected) {
  const normalize = (ids) => ids.map((id) => JSON.stringify(id)).sort();
  assert.deepEqual(normalize(actual), normalize(expected));
}

export function assertReport(report, admitted = [], pending = 0) {
  assertIds(report.admitted, admitted);
  assert.deepEqual(report.rejected, []);
  assert.equal(report.pending, pending);
}

export function assertErrorCode(error, code) {
  assert.ok(error instanceof StockfoilError);
  assert.ok(error instanceof Error);
  assert.equal(error.name, 'StockfoilError');
  assert.equal(error.code, code);
  assert.equal(typeof error.message, 'string');
  assert.ok(error.message.length > 0);
  return true;
}

export function throwsCode(fn, code) {
  assert.throws(fn, (error) => assertErrorCode(error, code));
}

export function graph(t, permissions = []) {
  const result = new CommitGraph();
  t.after(() => result.dispose());
  result.setWritePermissions(new Map(permissions.map((who) => [who.author, 'unbounded'])));
  return result;
}

export function events(batch) {
  return batch.changes.map(({ commit, newlyDelivered, visibility }) => [commit.hash, newlyDelivered, visibility]);
}

export function assertTopological(batch) {
  const positions = new Map(batch.changes.map(({ commit }, index) => [commit.hash, index]));
  assert.equal(positions.size, batch.changes.length, 'each commit appears once');
  for (const [index, { commit }] of batch.changes.entries()) {
    for (const parent of commit.parents) {
      if (positions.has(parent)) assert.ok(positions.get(parent) < index, 'parents precede children');
    }
  }
}

// Publicly observable state, including delivery and the received-item queue.
export function snapshot(graph) {
  const { added, retired } = graph.exportChanges(new ExportCursor());
  return {
    view: graph.viewPoint(), signed: graph.signedViewPoint(),
    changes: graph.changes(new MaterializationCursor()).changes,
    added, retired, pending: graph.pendingItems(), missing: graph.missingDependencies(),
  };
}

export function rejectsWithoutMutation(graph, operation, code) {
  const before = snapshot(graph);
  throwsCode(operation, code);
  assert.deepEqual(snapshot(graph), before);
}
