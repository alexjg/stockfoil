import test from 'node:test';
import assert from 'node:assert/strict';
import { ExportCursor } from 'stockfoil';
import { alice, atDepth, fragment, signedCommit, graph, copy } from './support.mjs';
import { encodeFixture } from './fixture-codec.mjs';

const prefixes = item => item.metadata.checkpoints.map(bytes => {
  assert.ok(bytes instanceof Uint8Array);
  assert.equal(bytes.byteLength, 12);
  return Buffer.from(bytes).toString('hex');
});
const prefix = commit => commit.hash.slice(0, 24);
const findHead = (items, head) => items.find(item => item.metadata.kind === 'fragment' && item.metadata.head === head.hash);

test('fragment exports contain owned sorted checkpoint prefixes, excluding boundary ancestry', t => {
  const source = graph(t, [alice]);
  const root = atDepth(alice, [], 1), boundary = atDepth(alice, [root.hash], 2);
  const a = atDepth(alice, [boundary.hash], 1), b = atDepth(alice, [a.hash], 1);
  const plain = atDepth(alice, [b.hash, root.hash]);
  const head = atDepth(alice, [plain.hash, boundary.hash, a.hash, root.hash], 2);
  source.ingest(encodeFixture(fragment(boundary, [root, boundary])));
  // root remains excluded boundary ancestry despite the bypass edges.
  source.ingest(encodeFixture(fragment(head, [a, b, plain, head], [boundary.hash])));
  const batch = source.exportChanges(new ExportCursor());
  const exported = findHead(batch.added, head), exportedBoundary = findHead(batch.added, boundary);
  const expected = [prefix(a), prefix(b)].sort();
  assert.deepEqual(prefixes(exported), expected);
  assert.deepEqual(prefixes(exportedBoundary), [prefix(root)]);
  const retained = findHead(source.exportChanges(new ExportCursor()).added, head);
  exported.metadata.checkpoints[0].fill(255);
  exported.metadata.checkpoints.reverse();
  exported.metadata.checkpoints.push(new Uint8Array(1));
  assert.deepEqual(prefixes(retained), expected);
  assert.deepEqual(prefixes(findHead(source.exportChanges(new ExportCursor()).added, head)), expected);
  source.setWritePermissions(new Map());
  assert.deepEqual(source.exportChanges(batch.cursor).added, []);
  source.commit(alice.author, new Uint8Array(1024 * 1024));
  source.dispose();
  assert.deepEqual(prefixes(retained), expected);

  // Checkpoint hints are returned metadata, never part of the ingest argument.
  // Even malformed or forged out-of-band hints cannot override native derivation.
  for (const checkpoints of [undefined, [new Uint8Array(12).fill(255)], [], [new Uint8Array(11)], ['not bytes']]) {
    const target = graph(t), received = copy(retained);
    if (checkpoints === undefined) delete received.metadata.checkpoints;
    else received.metadata.checkpoints = checkpoints;
    target.ingest(exportedBoundary.bytes);
    target.ingest(received.bytes);
    assert.deepEqual(prefixes(findHead(target.exportChanges(new ExportCursor()).added, head)), expected);
    assert.throws(() => target.ingest(received), { code: 'InvalidInput' });
  }
});

test('checkpoint-free fragments export empty hints; singleton announcements have no hint field', t => {
  const source = graph(t), head = atDepth(alice, [], 1);
  source.ingest(encodeFixture(signedCommit(head)));
  const announcement = source.exportChanges(new ExportCursor());
  assert.equal(announcement.added[0].metadata.kind, 'commit');
  assert.equal(Object.hasOwn(announcement.added[0].metadata, 'checkpoints'), false);
  source.ingest(encodeFixture(fragment(head, [head])));
  const batch = source.exportChanges(announcement.cursor);
  assert.equal(batch.added[0].metadata.kind, 'fragment');
  assert.deepEqual(batch.added[0].metadata.checkpoints, []);
  assert.deepEqual(batch.retired, [{ kind: 'commit', hash: head.hash }]);
});
