import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import * as api from 'stockfoil';
import {
  Ledger, Materialization, acknowledgeExport, encodeExportCursor, decodeExportCursor,
  inspectCheckpoint, verify, driveDecode, hex,
} from 'stockfoil';
import { alice, bob, chunkCodec, doc, fromHex, items, payloadAtDepth, signed, sync, text } from './support.mjs';

const plain = value => {
  assert.equal(Object.getPrototypeOf(value), Object.prototype);
  assert.deepEqual(structuredClone(value), value);
};

function drain(planner, ledger, apply = () => ({}), budget = 2) {
  let rounds = 0;
  while (!planner.isCurrent(ledger)) {
    let batch = planner.plan(ledger, budget);
    while (batch) {
      plain(batch);
      batch.events.forEach(plain);
      batch = planner.complete(ledger, structuredClone(batch), apply(batch.events) ?? {});
      assert.ok(++rounds < 1000);
    }
  }
}

test('public data are POJOs; implementation types are not exported', () => {
  for (const name of ['History', 'Graph', 'Queue', 'Prepared', 'ViewPoint', 'LedgerEngine', 'MaterializationEngine'])
    assert.equal(api[name], undefined);
  const d = doc();
  const draft = d.commit(text('draft'));
  plain(draft);
  plain(d.ledger.view());
  const signed = d.attachSignature(draft, alice.sign(draft.hash));
  plain(signed);
  const plan = d.export();
  plain(plan);
  plan.items.forEach(plain);
  assert.ok(Array.isArray(plan.cursor));
  const copy = structuredClone(plan);
  d.commit(text('later')); // captured data do not borrow the ledger
  assert.equal(copy.items.length, 1);
  assert.deepEqual(d.exportItem(copy.items[0]), d.exportItem(plan.items[0]));
  d.dispose();
  d.dispose();
  assert.throws(() => d.commit(text('disposed')), /disposed/);
});

test('synchronous local commits, fragments, canonical payloads and durable cursors', () => {
  const a = doc(), b = doc(bob);
  let tip = signed(a);
  const hashes = [tip];
  for (let i = 0; i < 3; i++) {
    tip = signed(a, alice, [tip], text('member ' + i));
    hashes.push(tip);
  }
  tip = signed(a, alice, [tip], payloadAtDepth(a.ledger, [tip], 'fragment'));
  hashes.push(tip);
  const draft = a.commit(text('unsigned'), [tip]);
  assert.deepEqual(a.ledger.drafts(), [draft]);
  assert.deepEqual(a.export().items.map(i => i.id.kind), ['fragment']);
  const cursor = sync(a, b);
  for (const hash of hashes) {
    assert.ok(b.ledger.lookup(hash).admitted);
    assert.deepEqual(b.payload(hash), a.payload(hash));
  }
  assert.equal(b.ledger.lookup(draft.hash), undefined);
  assert.deepEqual(b.ledger.parents(tip), [hashes.at(-2)]);
  assert.ok(b.ledger.isAncestor(hashes[0], tip));
  const restored = decodeExportCursor(encodeExportCursor(cursor));
  assert.deepEqual(restored, cursor);
  assert.equal(a.export(restored).items.length, 0);
  assert.deepEqual(a.attachSignature(draft, alice.sign(draft.hash)).admitted, [draft.hash]);
  assert.equal(sync(a, b, restored).length, 2);
  // Duplicate receipts retain original IDs and clear staging.
  b.receive(a.exportItem(a.export().items[0]));
  assert.equal(b.stagedItems, 0);
});

test('native Rust interop fixture is unchanged', () => {
  const fixture = JSON.parse(readFileSync(new URL('./fixtures/rust-items.json', import.meta.url)));
  const d = doc(bob, { readSize: 13, decodeCapacity: 5 });
  assert.equal(fixture.format, chunkCodec.format);
  assert.deepEqual(fromHex(fixture.author), alice.author);
  for (const item of fixture.items) {
    const result = d.receive(fromHex(item.bytes));
    assert.equal(result.kind, 'admitted');
    assert.equal(result.items[0].id.kind, item.kind);
    assert.deepEqual(result.items[0].id.head, fromHex(item.head));
  }
  for (const commit of fixture.commits)
    assert.deepEqual(d.payload(fromHex(commit.hash)), fromHex(commit.payload));
  const other = doc();
  sync(d, other);
  assert.deepEqual(other.ledger.heads('admitted'), d.ledger.heads('admitted'));
});

test('ledger parking releases receipts; eviction and duplicate staging are bounded', () => {
  const a = doc();
  const root = signed(a);
  const first = items(a);
  acknowledgeExport(first.plan, 0);
  const child = signed(a, alice, [root], text('child'));
  const second = items(a, first.plan.cursor).items[0];
  const b = doc(bob);
  assert.equal(b.receive(second).kind, 'waiting');
  assert.equal(b.receive(second).kind, 'waiting');
  assert.equal(b.ledger.parked, 2);
  assert.equal(b.stagedItems, 1);
  const result = b.receive(first.items[0]);
  assert.equal(result.kind, 'admitted');
  assert.equal(result.items.length, 3);
  assert.ok(b.ledger.lookup(child).admitted);
  assert.equal(b.stagedItems, 0);

  const capped = doc(bob, { maxParked: 1 });
  capped.receive(second);
  assert.equal(capped.receive(second).kind, 'evicted');
  assert.equal(capped.stagedItems, 1, 'one duplicate remains parked after eviction');
  capped.receive(first.items[0]);
  assert.deepEqual(capped.payload(child), text('child'));
  assert.equal(capped.stagedItems, 0);

  const c = doc(bob, { maxParked: 0 });
  const evicted = c.receive(second);
  assert.equal(evicted.kind, 'evicted');
  assert.deepEqual(evicted.evicted.head, child);
  assert.equal(c.ledger.parked, 0);
  assert.equal(c.stagedItems, 0);
});

test('caller-owned async verification, interleaved local commits, exact BigInts and checked decoding', async () => {
  const a = doc();
  const root = signed(a, alice, [], text('a'.repeat(300)));
  const wire = items(a).items[0];
  const ledger = new Ledger({ format: chunkCodec.format, author: bob.author, readSize: 7, decodeCapacity: 3 });
  const op = ledger.beginReceive();
  let reads = 0;
  await verify(op, {
    read: async (offset, max) => {
      reads++;
      assert.ok(max <= 7);
      // No Wasm borrow survives caller I/O.
      if (reads === 2) {
        const builder = ledger.beginCommit([]);
        builder.update(text('concurrent local'));
        ledger.commit(builder, 9n);
      }
      await Promise.resolve();
      return wire.subarray(offset, offset + max);
    },
    decoder: chunkCodec.decoder,
  });
  const big = 2n ** 60n + 7n;
  const result = ledger.admit(op, (_id, members) => members.map(() => big));
  assert.equal(result.items[0].members[0].payload, big);
  assert.equal(ledger.lookup(root).payload, big);
  assert.equal(ledger.length, 2);
  op.dispose(); // consumed, safe to dispose again
  assert.throws(() => op.next(), /disposed/);
  const decoder = ledger.decodeItem([[0, root]], { readSize: 1, decodeCapacity: 2 });
  const chunks = [];
  await driveDecode(decoder, {
    read: (offset, max) => wire.subarray(offset, offset + max),
    decoder: chunkCodec.decoder,
  }, (index, bytes) => { assert.equal(index, 0); chunks.push(bytes); });
  assert.deepEqual(new Uint8Array(Buffer.concat(chunks)), a.payload(root));
  decoder.dispose();
  const check = ledger.checkPayload(root);
  check.update(text('corrupt'));
  assert.throws(() => check.finish(), { code: 'CorruptPayload' });
  check.dispose();
});

test('reply validation preserves outstanding requests; host failures and abort are reported', async () => {
  const ledger = new Ledger({ format: chunkCodec.format, author: alice.author });
  const op = ledger.beginReceive();
  const next = op.next();
  plain(next);
  assert.equal(op.next().type, 'awaitReply');
  assert.throws(() => op.reply(next.ticket, { kind: 'bound', ids: [1n] }), { code: 'WrongKindReply' });
  assert.throws(() => op.reply(next.ticket + 1n, { kind: 'read', bytes: new Uint8Array(), eof: true }), { code: 'StaleReply' });
  assert.throws(() => op.reply(next.ticket, { kind: 'nonsense' }), { code: 'InvalidInput' });
  assert.equal(op.request().ticket, next.ticket);
  op.dispose();
  const failed = ledger.beginReceive();
  const cause = new Error('disk on fire');
  await assert.rejects(verify(failed, { read: () => { throw cause; }, decoder: chunkCodec.decoder }),
    error => error.code === 'Storage' && error.cause === cause);
  failed.dispose();
  const aborted = ledger.beginReceive();
  const controller = new AbortController();
  controller.abort(new Error('cancelled'));
  await assert.rejects(verify(aborted, {}, { signal: controller.signal }), /cancelled/);
  aborted.dispose();
  const builder = ledger.beginCommit();
  builder.update(text('x'));
  assert.throws(() => ledger.commit(builder, 5), { code: 'InvalidInput' });
  assert.throws(() => ledger.commit(builder, 2n ** 64n), { code: 'InvalidInput' });
  ledger.commit(builder, 2n ** 64n - 1n); // validation did not consume builder
});

test('independent materializations deliver invisible metadata and follow document policy', () => {
  const a = doc(), b = doc(bob);
  const root = signed(a);
  sync(a, b);
  const hidden = signed(b, bob, [root], text('hidden'));
  sync(b, a);
  const child = signed(a, alice, [hidden], text('child'));
  const working = new Materialization({ supportsVisibilityChanges: true });
  const pinned = new Materialization({ follow: { kind: 'pinned', heads: [root] }, supportsVisibilityChanges: true });
  const seen = [];
  drain(working, a.ledger, events => { seen.push(...events); });
  const delivered = seen.filter(e => e.kind === 'deliver');
  assert.deepEqual(delivered.map(e => e.hash), [root, hidden, child]);
  assert.deepEqual(delivered.map(e => e.active), [true, false, true]);
  const pinEvents = [];
  drain(pinned, a.ledger, events => { pinEvents.push(...events); });
  assert.deepEqual(pinEvents.filter(e => e.kind === 'deliver').map(e => e.active), [true, false, false]);
  a.ledger.setPermissions([[alice.author, 'unbounded'], [bob.author, 'unbounded']]);
  const enabled = [];
  drain(working, a.ledger, events => { enabled.push(...events); });
  assert.deepEqual(enabled, [{ kind: 'activate', hash: hidden }]);
  a.ledger.setPermissions([[alice.author, [root]], [bob.author, 'unbounded']]);
  const retracted = [];
  drain(working, a.ledger, events => { retracted.push(...events); });
  assert.deepEqual(retracted, [{ kind: 'retract', hash: child }]);
  // An unlisted rule admits authors without their own entry; listed rules still win.
  a.ledger.setPermissions([[alice.author, [root]]], 'unbounded');
  assert.equal(a.ledger.unlistedPermission(), 'unbounded');
  assert.equal(a.ledger.view().unlisted, 'unbounded');
  const opened = [];
  drain(working, a.ledger, events => { opened.push(...events); });
  assert.deepEqual(opened, []); // bob still participates; child stays retracted
  a.ledger.setPermissions([]);
  assert.equal(a.ledger.unlistedPermission(), 'denied');
  assert.throws(() => a.ledger.setPermissions([], 'everyone'), { code: 'InvalidInput' });
});

test('POJO completion tokens reject mutation, foreign planners, replay and preflight failures', () => {
  const d = doc();
  signed(d);
  const p = new Materialization(), other = new Materialization();
  const batch = p.plan(d.ledger);
  const theirs = other.plan(d.ledger);
  assert.throws(() => p.plan(d.ledger), { code: 'Protocol' });
  assert.throws(() => p.complete(d.ledger, theirs), { code: 'StaleBatch' });
  const modified = structuredClone(batch);
  modified.events[0].active = false;
  assert.throws(() => p.complete(d.ledger, modified), { code: 'StaleBatch' });
  const wrong = doc();
  assert.throws(() => p.complete(wrong.ledger, batch), { code: 'WrongHistory' });
  assert.throws(() => p.complete(d.ledger, batch, { wanted: [new Uint8Array(32)] }), { code: 'UnknownCommit' });
  p.complete(d.ledger, structuredClone(batch));
  assert.throws(() => p.complete(d.ledger, batch), { code: 'StaleBatch' });
});

test('rejections, cascading corrections, wanted payloads, reset and checkpoint inspection', () => {
  const d = doc();
  const root = signed(d);
  const child = signed(d, alice, [root], text('child'));
  const p = new Materialization({ supportsVisibilityChanges: true });
  let batch = p.plan(d.ledger, 10);
  batch = p.complete(d.ledger, batch, { rejected: [{ hash: root, reason: 'bad', cascade: true }], wanted: [child] });
  assert.deepEqual(batch.events, [{ kind: 'retract', hash: child }, {
    kind: 'payload', hash: child, payload: d.ledger.lookup(child).payload,
  }]);
  assert.equal(p.complete(d.ledger, batch), undefined);
  const info = inspectCheckpoint(p.checkpoint);
  plain(info);
  assert.deepEqual(info.rejected, [{ hash: root, cascade: true }]);
  const resumed = new Materialization({ checkpoint: p.checkpoint, supportsVisibilityChanges: true });
  assert.equal(resumed.plan(d.ledger), undefined);
  assert.throws(() => inspectCheckpoint(new Uint8Array([1, 2])), { code: 'Malformed' });
  const reset = new Materialization();
  drain(reset, d.ledger);
  d.ledger.setPermissions([]);
  assert.equal(reset.plan(d.ledger).events[0].kind, 'reset');
});

test('partial checkpoint resumes canonically on a rebuilt ledger with later edits', () => {
  const d = doc();
  const roots = Array.from({ length: 8 }, (_, i) => signed(d, alice, [], text('root ' + i)));
  const p = new Materialization({ supportsVisibilityChanges: true });
  const first = p.plan(d.ledger, 3);
  const prefix = first.events.map(e => hex(e.hash));
  p.complete(d.ledger, first);
  const saved = p.checkpoint;
  assert.equal(inspectCheckpoint(saved).partial, true);
  const rebuilt = doc();
  for (const wire of items(d).items.reverse()) rebuilt.receive(wire);
  const later = signed(rebuilt, alice, [], text('later'));
  const resumed = new Materialization({ checkpoint: saved, supportsVisibilityChanges: true });
  const suffix = [];
  drain(resumed, rebuilt.ledger, events => { suffix.push(...events.filter(e => e.kind === 'deliver').map(e => hex(e.hash))); }, 2);
  const canonical = [];
  drain(new Materialization(), d.ledger, events => { canonical.push(...events.filter(e => e.kind === 'deliver').map(e => hex(e.hash))); });
  assert.deepEqual(prefix.concat(suffix.slice(0, 5)), canonical);
  assert.equal(suffix.at(-1), hex(later));
  assert.equal(new Set(prefix.concat(suffix)).size, roots.length + 1);
});

test('export cursors canonicalize sets and reject malformed plain input', () => {
  const a = { kind: 'commit', head: new Uint8Array(32).fill(1) };
  const b = { kind: 'fragment', head: new Uint8Array(32).fill(2),
    boundary: [new Uint8Array(32).fill(4), new Uint8Array(32).fill(3)] };
  const bytes = encodeExportCursor([b, a, a]);
  assert.deepEqual(encodeExportCursor(decodeExportCursor(bytes)), bytes);
  assert.equal(decodeExportCursor(bytes).length, 2);
  assert.throws(() => encodeExportCursor([{ kind: 'commit', head: new Uint8Array(2) }]),
    { code: 'InvalidInput' });
  assert.throws(() => decodeExportCursor(new Uint8Array([1])), { code: 'Malformed' });
});

test('in-memory bytes are owned, and simultaneous admissions never retain ledger borrows', async () => {
  const a = doc();
  const payload = text('original');
  const draft = a.commit(payload);
  payload.fill(0);
  a.attachSignature(draft, alice.sign(draft.hash));
  const read = a.payload(draft.hash);
  read.fill(0);
  assert.deepEqual(a.payload(draft.hash), text('original'));
  for (let i = 0; i < 10; i++) signed(a, alice, [], text('root ' + i));

  const ledger = new Ledger({ format: chunkCodec.format, author: bob.author });
  const wires = items(a).items;
  let nextId = 2n ** 60n;
  await Promise.all(wires.map(async wire => {
    const op = ledger.beginReceive({ readSize: 7, decodeCapacity: 3 });
    try {
      await verify(op, {
        read: async (offset, max) => {
          await new Promise(resolve => setImmediate(resolve));
          assert.ok(ledger.length >= 0, 'queries interleave with all admissions');
          return wire.subarray(offset, offset + max);
        },
        decoder: chunkCodec.decoder,
      });
      ledger.admit(op, (_id, hashes) => hashes.map(() => nextId++));
    } finally { op.dispose(); }
  }));
  assert.deepEqual(ledger.heads('admitted'), a.ledger.heads('admitted'));
});

test('buffer bounds reject unsafe/Wasm-overflow sizes while byte limits allow multi-GiB items', () => {
  const ledger = new Ledger({ format: chunkCodec.format, author: alice.author,
    limits: { maxCanonicalBytes: 2 ** 40, maxItemBytes: 2 ** 40 } });
  ledger.beginReceive().dispose();
  for (const readSize of [2 ** 32, 2 ** 53, -1, NaN]) {
    assert.throws(() => ledger.beginReceive({ readSize }), { code: 'InvalidInput' });
  }
});

test('owned ancestry, wrong-history protection, pinned edits and markLocal echo suppression', () => {
  const d = doc();
  const root = signed(d);
  const p = new Materialization({ follow: { kind: 'pinned', heads: [root] } });
  drain(p, d.ledger);
  const before = d.ledger.ancestry([root]);
  const draft = d.commit(text('local'), p.heads);
  p.markLocal(d.ledger, draft);
  assert.ok(p.isCurrent(d.ledger));
  assert.deepEqual(p.follow, { kind: 'pinned', heads: [draft.hash] });
  assert.equal(p.plan(d.ledger), undefined);
  const after = d.ledger.ancestry([draft.hash]);
  assert.equal(before.contains(d.ledger, draft.hash), false);
  assert.equal(after.participates(d.ledger, draft.hash), true);
  assert.deepEqual(after.difference(before, d.ledger), [draft.hash]);
  assert.throws(() => after.contains(doc().ledger, root), { code: 'WrongHistory' });
  before.dispose(); after.dispose();
});
