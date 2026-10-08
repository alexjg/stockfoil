import assert from 'node:assert/strict';
import test from 'node:test';
import { MaterializationCursor } from 'stockfoil';
import { registerCodec } from '../codec.mjs';
import { Replica } from '../register.mjs';

async function pair() {
  const alice = new Replica(), bob = new Replica();
  await alice.trust(bob.author);
  await bob.trust(alice.author);
  return { alice, bob };
}

test('concurrent writes survive until a later write supersedes them', async () => {
  const { alice, bob } = await pair();
  await alice.set('color', 'red');
  await alice.syncTo(bob);
  assert.deepEqual(bob.get('color'), ['red']);

  // Concurrent edits on both replicas.
  await alice.set('color', 'green');
  await bob.set('color', 'blue');
  await alice.syncTo(bob);
  await bob.syncTo(alice);
  assert.deepEqual(alice.get('color'), ['blue', 'green']);
  assert.deepEqual(bob.get('color'), ['blue', 'green']);

  // A write that has seen both supersedes both.
  await bob.set('color', 'purple');
  await bob.syncTo(alice);
  assert.deepEqual(alice.get('color'), ['purple']);
  assert.deepEqual(alice.graph.history.heads('admitted'), bob.graph.history.heads('admitted'));
  // Only what is new is sent again.
  assert.equal(bob.graph.export(bob.exportCursors.get(alice)).length, 0);
});

test('permissions decide participation without losing delivered history', async () => {
  const { alice, bob } = await pair();
  await alice.set('title', 'draft');
  await alice.syncTo(bob);
  await bob.set('title', 'final');
  await bob.syncTo(alice);
  assert.deepEqual(alice.get('title'), ['final']);

  // Revoking bob hides his write and reveals the one it superseded.
  await alice.trust(bob.author, 'denied');
  assert.deepEqual(alice.get('title'), ['draft']);
  await alice.trust(bob.author, 'unbounded');
  assert.deepEqual(alice.get('title'), ['final']);
});

test('items move through the codec in fragments and the cursor persists', async () => {
  const { alice, bob } = await pair();
  // Write until some commit hash has a leading zero byte (about 1 in 256):
  // that commit heads a fragment carrying the writes beneath it.
  const kinds = () => {
    const plan = alice.graph.export();
    return Array.from({ length: plan.length }, (_, i) => plan.id(i).kind);
  };
  let i = 0;
  while (!kinds().includes('fragment')) await alice.set(`k${i % 3}`, `v${i++}`);
  await alice.set('k0', 'last');
  await alice.syncTo(bob);
  assert.deepEqual(bob.get('k0'), alice.get('k0'));
  assert.deepEqual(bob.get('k0'), ['last']);

  const saved = MaterializationCursor.fromBytes(bob.cursor.toBytes());
  const changes = bob.graph.history.changes(saved, bob.graph.history.viewPoint(bob.permissions.values()));
  assert.equal(changes.remaining, 0);
});

test('the decoder streams with one-byte input and output', () => {
  const payloads = [new TextEncoder().encode('["a","b"]'), new Uint8Array(), new Uint8Array([7, 8])];
  const body = registerCodec.encode(null, payloads);
  const decoder = registerCodec.decoder({ members: payloads });
  const members = [[]];
  let offset = 0;
  for (;;) {
    const input = body.subarray(offset, offset + 1);
    const step = decoder.step(input, offset + 1 >= body.length, 1);
    offset += step.consumed;
    members.at(-1).push(...step.output);
    if (step.status === 'endMember') members.push([]);
    if (step.status === 'finished') break;
  }
  members.pop();
  assert.deepEqual(members.map(m => Uint8Array.from(m)), payloads);
});
