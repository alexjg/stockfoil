import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import {
  drive, ExportCursor, Graph, History, MaterializationCursor, MemoryStorage, ViewPoint, blobHash,
} from 'stockfoil';
import {
  alice, bob, chunkCodec, fromHex, items, payloadAtDepth, settled, signed, SlowStorage, sync, text,
} from './support.mjs';

const graph = (storage) => new Graph(chunkCodec, storage);

test('round trip between two JS graphs, with a fragment and a draft left behind', async () => {
  const a = graph(), b = graph();
  let tip = await signed(a, alice, [], text('root'));
  const hashes = [tip];
  for (let i = 0; i < 3; i++) {
    tip = await signed(a, alice, [tip], text(`member ${i}`.repeat(1000)));
    hashes.push(tip);
  }
  tip = await signed(a, alice, [tip], payloadAtDepth(a.history, alice, [tip], 'head'));
  hashes.push(tip);
  const draft = await a.commit(alice.author, [tip], text('unsigned'));
  assert.deepEqual(a.history.drafts(), [draft.hash]);

  const plan = a.export();
  assert.deepEqual([...Array(plan.length).keys()].map(i => plan.id(i).kind), ['fragment']);
  const cursor = await sync(a, b);
  for (const hash of hashes) {
    assert.ok(b.history.isAdmitted(hash));
    assert.deepEqual(await b.payload(b.history.lookup(hash).payload), await a.payload(a.history.lookup(hash).payload));
  }
  assert.equal(b.history.lookup(draft.hash), undefined);
  assert.deepEqual(b.history.heads('admitted'), [tip]);
  assert.deepEqual(b.history.parents(tip), [hashes[3]]);
  assert.ok(b.history.isAncestor(hashes[0], tip));

  // The cursor survives serialization; nothing new is planned against it.
  const restored = ExportCursor.fromBytes(cursor.toBytes());
  assert.deepEqual(restored.toBytes(), cursor.toBytes());
  assert.equal(a.export(restored).length, 0);

  // Signing the draft admits it; only it is exported next.
  assert.deepEqual(a.sign(alice.author, draft.hash, alice.sign(draft.signingBytes)), [{ kind: 'commit', head: draft.hash }]);
  const next = await sync(a, b, restored);
  assert.ok(b.history.isAdmitted(draft.hash));
  assert.equal(next.length, 2);
});

test('admits items exported by the native Rust graph', async () => {
  const fixture = JSON.parse(readFileSync(new URL('./fixtures/rust-items.json', import.meta.url)));
  assert.equal(fixture.format, chunkCodec.format);
  // The JS signer derives the same Ed25519 key as the Rust tests' `key(1)`.
  assert.deepEqual(fromHex(fixture.author), alice.author);
  const js = new Graph(chunkCodec, new MemoryStorage(), { readSize: 13, decodeCapacity: 5 });
  for (const item of fixture.items) {
    const receipt = await js.receive(fromHex(item.bytes));
    assert.equal(receipt.id.kind, item.kind);
    assert.deepEqual(receipt.id.head, fromHex(item.head));
  }
  for (const commit of fixture.commits) {
    const info = js.history.lookup(fromHex(commit.hash));
    assert.ok(info.admitted);
    assert.deepEqual(info.author, alice.author);
    assert.deepEqual(await js.payload(info.payload), fromHex(commit.payload));
  }
  // And re-exported from JS to another JS graph.
  const other = graph();
  await sync(js, other);
  assert.deepEqual(other.history.heads('admitted'), js.history.heads('admitted'));
});

test('concurrent admissions with delayed storage, interleaved with local commits', async () => {
  const a = graph();
  const roots = [];
  for (let i = 0; i < 12; i++) roots.push(await signed(a, i % 2 ? alice : bob, [], text(`root ${i} `.repeat(200))));
  const { items: bytes } = await items(a);
  assert.equal(bytes.length, 12);

  const storage = new SlowStorage();
  const b = new Graph(chunkCodec, storage, { readSize: 64, decodeCapacity: 32, quantum: 16 });
  const local = (async () => {
    let tip = await signed(b, alice, [], text('local root'));
    for (let i = 0; i < 5; i++) tip = await signed(b, alice, [tip], text(`local ${i}`));
    return tip;
  })();
  const receipts = await Promise.all([...bytes.map(item => b.receive(item)), local]);
  for (const root of roots) assert.ok(b.history.isAdmitted(root));
  assert.ok(b.history.isAdmitted(receipts.at(-1)));
  assert.equal(b.history.length, 18);
  assert.ok(storage.reads > 12 * 10, 'small reads were interleaved');
});

test('stale, duplicate, and wrong-kind replies are rejected without disturbing the operation', async () => {
  const a = graph();
  await signed(a, alice, [], text('only'));
  const [bytes] = (await items(a)).items;
  const history = new History();
  const item = blobHash(bytes);
  const admission = history.receive(item, { format: chunkCodec.format });
  const next = admission.next();
  assert.equal(next.type, 'request');
  assert.deepEqual(next.request, { kind: 'read', item, offset: 0, max: 65536 });
  assert.equal(admission.next().type, 'awaitReply');
  assert.deepEqual(admission.request().ticket, next.ticket);

  const read = { kind: 'read', bytes, eof: false };
  assert.throws(() => admission.reply(next.ticket, { kind: 'bound', ids: [1n] }), { code: 'WrongKindReply' });
  assert.throws(() => admission.reply(next.ticket + 1n, read), { code: 'StaleReply' });
  assert.throws(() => admission.reply(next.ticket, { kind: 'nonsense' }), { code: 'InvalidInput' });
  admission.reply(next.ticket, read);
  assert.throws(() => admission.reply(next.ticket, read), { code: 'StaleReply' });

  // The admission still completes through the ordinary driver.
  const storage = new MemoryStorage();
  storage.put(bytes);
  const host = new Graph(chunkCodec, storage).host;
  const receipt = await drive(history, admission, host);
  assert.equal(receipt.members.length, 1);
  assert.equal(admission.counts().bytesRead, bytes.length);
});

test('protocol violations and host failures fail the admission', async () => {
  const a = graph();
  await signed(a, alice, [], text('only'));
  const [bytes] = (await items(a)).items;
  const history = new History();
  const admission = history.receive(blobHash(bytes), { format: chunkCodec.format, readSize: 4 });
  const { ticket } = admission.next();
  assert.throws(() => admission.reply(ticket, { kind: 'read', bytes: bytes.subarray(0, 5), eof: false }), { code: 'Protocol' });
  const failed = admission.next();
  assert.equal(failed.type, 'failed');
  assert.equal(failed.error.code, 'Protocol');

  const broken = new Error('disk on fire');
  const host = { read: async () => { throw broken; }, bind: () => [], decoder: () => null };
  await assert.rejects(
    drive(history, history.receive(blobHash(bytes), { format: chunkCodec.format }), host),
    (e) => e.code === 'Storage' && e.cause === broken,
  );
  assert.equal(history.length, 0);
});

test('an admission waits for its dependency and retries when the epoch changes', async () => {
  const a = graph();
  const root = await signed(a, alice, [], text('root'));
  const { items: [first], plan } = await items(a);
  plan.acknowledge(0);
  const child = await signed(a, alice, [root], text('child'));
  const { items: [second] } = await items(a, plan.cursor());

  const b = graph();
  let advances = 0;
  const advance = b.history.advance.bind(b.history);
  b.history.advance = (admission) => { advances++; return advance(admission); };
  const waiting = b.receive(second);
  assert.equal(await settled(waiting), 'pending');
  assert.equal(advances, 1);
  // An unrelated local commit changes the epoch; the admission retries and keeps waiting.
  await signed(b, bob, [], text('unrelated'));
  assert.equal(await settled(waiting), 'pending');
  assert.ok(advances > 1, 'retried after each epoch change');
  assert.equal(b.history.isAdmitted(child), false);

  await b.receive(first);
  const receipt = await waiting;
  assert.deepEqual(receipt.members.map(m => m.hash), [child]);
  assert.ok(b.history.isAdmitted(child));

  // A waiting drive can be abandoned.
  const c = graph();
  const controller = new AbortController();
  const abandoned = c.receive(second, { signal: controller.signal });
  assert.equal(await settled(abandoned), 'pending');
  controller.abort(new Error('gave up'));
  await assert.rejects(abandoned, /gave up/);
});

test('materialization through change batches and permission changes', async () => {
  const g = graph();
  const a1 = await signed(g, alice, [], text('a1'));
  const b1 = await signed(g, bob, [a1], text('b1'));
  const a2 = await signed(g, alice, [b1], text('a2'));
  const draft = (await g.commit(alice.author, [a2], text('draft'))).hash;

  const materialize = (cursor, target, max) => {
    const changes = g.history.changes(cursor, target);
    const records = [];
    for (;;) {
      const batch = g.history.nextChanges(changes, max);
      assert.ok(batch.records.length <= max);
      records.push(...batch.records);
      if (batch.complete) return { records, cursor: batch.cursor };
      assert.equal(batch.cursor, undefined);
    }
  };
  const summary = (records) => records.map(r => [Buffer.from(r.hash).toString('hex').slice(0, 8), r.newlyDelivered, r.visibility]);
  const short = (h) => Buffer.from(h).toString('hex').slice(0, 8);

  const aliceOnly = new Map([[alice.author, 'unbounded']]);
  const first = materialize(new MaterializationCursor(), g.history.viewPoint(aliceOnly), 2);
  assert.deepEqual(summary(first.records), [
    [short(a1), true, 'after'], [short(b1), true, 'neither'],
    [short(a2), true, 'after'], [short(draft), true, 'after'],
  ]);
  for (const r of first.records) {
    assert.deepEqual(await g.payload(r.payload), text(['a1', 'b1', 'a2', 'draft'][first.records.indexOf(r)]));
  }

  // The cursor round-trips through bytes.
  const saved = MaterializationCursor.fromBytes(first.cursor.toBytes());
  assert.deepEqual(saved.deliveryHeads(), [draft]);
  assert.deepEqual(saved.viewPoint().heads(), [draft]);

  // Granting bob makes b1 participate; nothing is newly delivered.
  const both = [[alice.author, 'unbounded'], [bob.author, 'unbounded']];
  const second = materialize(saved, g.history.signedViewPoint(both), 10);
  assert.deepEqual(summary(second.records), [[short(b1), false, 'after'], [short(draft), false, 'before']]);

  // A bounded frontier for alice hides a2; an explicit view point works too.
  const view = new ViewPoint([a2], [[alice.author, [a1]], [bob.author, 'unbounded']]);
  assert.deepEqual(new Map(view.permissions().map(([a, p]) => [short(a), p])),
    new Map([[short(alice.author), [a1]], [short(bob.author), 'unbounded']]));
  const third = materialize(second.cursor, view, 10);
  assert.deepEqual(summary(third.records), [[short(a2), false, 'before']]);
});

test('payload IDs beyond 2^53 stay exact BigInts', async () => {
  const big = 2n ** 60n + 7n;
  const a = new Graph(chunkCodec, new MemoryStorage({ firstId: big }));
  const root = await signed(a, alice, [], text('root'));
  assert.equal(a.history.lookup(root).payload, big);
  const child = await a.commit(alice.author, [root], text('child'));
  assert.equal(a.history.lookup(child.hash).payload, big + 1n);

  const b = new Graph(chunkCodec, new MemoryStorage({ firstId: 2n ** 64n - 2n }));
  const [receipt] = await Promise.all([b.receive((await items(a)).items[0])]);
  assert.deepEqual(receipt.members.map(m => m.payload), [2n ** 64n - 2n]);
  assert.deepEqual(await b.payload(2n ** 64n - 2n), text('root'));

  const changes = b.history.changes(new MaterializationCursor(), b.history.viewPoint([[alice.author, 'unbounded']]));
  assert.equal(b.history.nextChanges(changes, 10).records[0].payload, 2n ** 64n - 2n);

  const plan = a.export();
  assert.equal(a.history.exportItem(plan, 0, chunkCodec.format).members[0].payload, big);
  assert.throws(() => a.history.publishCommit(child, 2n ** 64n), { code: 'InvalidInput' });
  assert.throws(() => a.history.publishCommit(child, 5), { code: 'InvalidInput' });
});
