import test from 'node:test';
import assert from 'node:assert/strict';
import { ExportCursor } from 'stockfoil';
import { Replica } from '../transport.mjs';
import { MvRegisterMap } from '../register.mjs';
import { checkpoint } from './support.mjs';

const hex = bytes => Buffer.from(bytes).toString('hex');
function replica(t) {
  // Local storage only: these tests deliberately never connect to a server.
  const result = new Replica({ peerId: '00'.repeat(32), documentId: '01'.repeat(32) });
  t.after(() => result.close());
  return result;
}

test('one local batch stores loose commits and nonempty checkpoint prefixes', async t => {
  const doc = new MvRegisterMap(); t.after(() => doc.dispose());
  const interior = checkpoint(doc, 1);
  checkpoint(doc, 2);
  checkpoint(doc, 0, 'tip');
  const expected = doc.graph.exportChanges(new ExportCursor()).added;
  const exported = expected.find(item => item.metadata.kind === 'fragment').metadata;
  assert.deepEqual(exported.checkpoints.map(hex), [interior.slice(0, 24)]);

  const client = replica(t);
  const store = client.node.storeBuiltBatch.bind(client.node);
  const calls = [];
  t.mock.method(client.node, 'storeBuiltBatch', async (id, commits, fragments) => {
    calls.push([commits.length, fragments.length]);
    return store(id, commits, fragments);
  });
  assert.deepEqual(await client.publish(doc), expected);
  assert.deepEqual(calls, [[1, 1]]);
  assert.deepEqual((await client.blobs()).map(hex).sort(), expected.map(item => hex(item.bytes)).sort());

  const stored = await client.node.getFragments(client.id);
  assert.equal(stored.length, 1);
  const checkpoints = stored[0].checkpoints;
  try {
    assert.deepEqual(checkpoints.map(checkpoint => hex(checkpoint.toBytes())), exported.checkpoints.map(hex));
  } finally {
    for (const checkpoint of checkpoints) checkpoint.free();
    for (const item of stored) item.free();
  }
  assert.deepEqual(await client.publish(doc), []);
  assert.deepEqual(calls, [[1, 1], [0, 0]]);
});

test('failed publication retains its cursor and safely retries already stored items', async t => {
  const doc = new MvRegisterMap();
  t.after(() => doc.dispose());
  doc.set('title', 'retry');
  const client = replica(t);
  const previous = client.published;
  const store = client.node.storeBuiltBatch.bind(client.node);
  let fail = true;
  t.mock.method(client.node, 'storeBuiltBatch', async (...args) => {
    await store(...args);
    // Model a failure after storage has changed, not an atomic rollback.
    if (fail) { fail = false; throw new Error('injected storage failure'); }
  });
  await assert.rejects(client.publish(doc), /injected storage failure/);
  assert.equal(client.published, previous);
  const stored = (await client.blobs()).map(hex).sort();
  assert.equal(stored.length, 1);
  assert.equal((await client.publish(doc)).length, 1);
  assert.notEqual(client.published, previous);
  assert.deepEqual((await client.blobs()).map(hex).sort(), stored);
  assert.deepEqual(await client.publish(doc), []);
});
