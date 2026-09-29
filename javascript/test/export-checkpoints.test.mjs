import test from 'node:test';
import assert from 'node:assert/strict';
import { ExportCursor } from 'stockfoil';
import { alice, atDepth, fragment, signedCommit, graph, copy } from './support.mjs';

const prefixes = item => item.checkpoints.map(bytes => {
  assert.ok(bytes instanceof Uint8Array);
  assert.equal(bytes.byteLength, 12);
  return Buffer.from(bytes).toString('hex');
});
const prefix = commit => commit.hash.slice(0, 24);

test('fragment exports contain owned, sorted checkpoint prefixes, excluding boundary ancestry', t => {
  const source = graph(t, [alice]);
  const root = atDepth(alice, [], 1);
  const boundary = atDepth(alice, [root.hash], 2);
  const a = atDepth(alice, [boundary.hash], 1);
  const b = atDepth(alice, [a.hash], 1);
  const plain = atDepth(alice, [b.hash, root.hash]);
  const head = atDepth(alice, [plain.hash, boundary.hash, a.hash, root.hash], 2);
  source.ingest(fragment(boundary, [root, boundary]));
  // root is reachable along bypasses but remains excluded boundary ancestry.
  source.ingest(fragment(head, [a, b, plain, head], [boundary.hash]));
  const batch = source.exportChanges(new ExportCursor());
  const exported = batch.added.find(item => item.head === head.hash);
  const exportedBoundary = batch.added.find(item => item.head === boundary.hash);
  const expected = [prefix(a), prefix(b)].sort();
  assert.deepEqual(prefixes(exported), expected);
  assert.deepEqual(prefixes(exportedBoundary), [prefix(root)]);

  const retained = source.exportChanges(new ExportCursor()).added.find(item => item.head === head.hash);
  exported.checkpoints[0].fill(255);
  exported.checkpoints.reverse();
  exported.checkpoints.push(new Uint8Array(1));
  assert.deepEqual(prefixes(retained), expected);
  assert.deepEqual(prefixes(source.exportChanges(new ExportCursor()).added.find(item => item.head === head.hash)), expected);
  source.setWritePermissions(new Map());
  assert.deepEqual(source.exportChanges(batch.cursor).added, []);
  source.commit(alice.author, new Uint8Array(1024 * 1024)); // May grow Wasm memory.
  source.dispose();
  assert.deepEqual(prefixes(retained), expected);

  // Well-formed hints are not evidence: false, empty, or absent hints cannot
  // override derived metadata. Original metadata and signatures are still checked.
  for (const checkpoints of [undefined, [new Uint8Array(12).fill(255)], []]) {
    const target = graph(t);
    const received = copy(retained);
    if (checkpoints === undefined) delete received.checkpoints;
    else received.checkpoints = checkpoints;
    target.ingest(exportedBoundary);
    target.ingest(received);
    const reexported = target.exportChanges(new ExportCursor()).added.find(item => item.head === head.hash);
    assert.deepEqual(prefixes(reexported), expected);
    received.signatures[0].signature[0] ^= 1;
    assert.throws(() => target.ingest(received), { code: 'InvalidSignature' });
  }
});

test('malformed checkpoint byte lengths fail before admission or queue mutation', t => {
  const target = graph(t);
  const head = atDepth(alice, [], 1);
  for (const checkpoints of [[new Uint8Array(11)], [new Uint8Array(13)], ['not bytes']]) {
    const item = fragment(head, [head]);
    item.checkpoints = checkpoints;
    assert.throws(() => target.ingest(item), { code: 'InvalidInput' });
    assert.equal(target.hasCommit(head.hash), false);
    assert.equal(target.pendingItems(), 0);
  }
});

test('checkpoint-free fragments export an empty array; standalone announcements have no field', t => {
  const source = graph(t);
  const head = atDepth(alice, [], 1);
  source.ingest(signedCommit(head));
  const announcement = source.exportChanges(new ExportCursor());
  assert.equal(announcement.added[0].kind, 'commit');
  assert.equal(Object.hasOwn(announcement.added[0], 'checkpoints'), false);
  source.ingest(fragment(head, [head]));
  const batch = source.exportChanges(announcement.cursor);
  assert.equal(batch.added[0].kind, 'fragment');
  assert.deepEqual(batch.added[0].checkpoints, []);
  assert.deepEqual(batch.retired, [{ kind: 'commit', hash: head.hash }]);
});
