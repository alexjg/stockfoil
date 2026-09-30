import assert from 'node:assert/strict';
import { sign } from 'node:crypto';
import { encodeAssignment, registerCodec } from '../codec.mjs';
import { ExportCursor } from 'stockfoil';

// Locate known application/signature bytes, never parse Stockfoil's envelope.
export function corruptKnownBytes(encoded, needle) {
  const offset = Buffer.from(encoded).indexOf(needle);
  assert.notEqual(offset, -1, 'known bytes must occur in exported item');
  const copy = encoded.slice(); copy[offset + needle.length - 1] ^= 1; return copy;
}
export function forgeries(doc, hash, assignment) {
  const items = doc.graph.exportChanges(new ExportCursor()).added;
  const { metadata, bytes } = items.find(({ metadata: m }) => (m.kind === 'commit' ? m.hash : m.head) === hash);
  // A depth-positive root may be represented as a one-member fragment.
  const body = metadata.kind === 'commit' ? encodeAssignment(assignment)
    : registerCodec.encodeFragment({ members: [{ author: doc.author, parents: [] }] }, [assignment]);
  const signature = sign(null, Buffer.from(hash, 'hex'), doc.privateKey);
  return [corruptKnownBytes(bytes, signature), corruptKnownBytes(bytes, body)];
}
export function checkpoint(doc, depth, key = 'checkpoint') {
  // Mine read-only, then exercise the ordinary typed commit/sign/export path.
  for (let nonce = 0; nonce < 10_000_000; nonce++) {
    const value = String(nonce);
    const { hash } = doc.graph.describeCommit(doc.author, { key, value });
    if (hash.startsWith('00'.repeat(depth)) && hash.slice(depth * 2, depth * 2 + 2) !== '00') return doc.set(key, value);
  }
  assert.fail('could not mine bounded checkpoint fixture');
}
