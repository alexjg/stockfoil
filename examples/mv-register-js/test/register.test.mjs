import test from 'node:test';
import assert from 'node:assert/strict';
import { ExportCursor, StockfoilError } from 'stockfoil';
import { MvRegisterMap } from '../register.mjs';
import { registerCodec, encodeAssignment, decodeAssignment } from '../codec.mjs';
import { forgeries } from './support.mjs';

function make(t) { const doc = new MvRegisterMap(); t.after(() => doc.dispose()); return doc; }
function transfer(from, to) {
  const items = from.graph.exportChanges(new ExportCursor()).added;
  to.ingest([...items].reverse().map(item => item.bytes));
}

test('retained assignments, concurrency, revocation, historical reads', t => {
  const alice = make(t), bob = make(t);
  const permissions = new Map([[alice.author, 'unbounded'], [bob.author, 'unbounded']]);
  alice.setWritePermissions(permissions); bob.setWritePermissions(permissions);
  alice.set('title', 'before'); transfer(alice, bob);
  const before = alice.viewPoint();
  alice.set('title', 'Hello'); bob.set('title', 'Howdy');
  transfer(alice, bob); transfer(bob, alice);
  for (const doc of [alice, bob]) assert.deepEqual(doc.get('title'), ['Hello', 'Howdy']);
  assert.deepEqual(alice.getAt(before, 'title'), ['before']);
  assert.deepEqual(alice.get('title'), ['Hello', 'Howdy']);
  bob.set('title', 'overwritten'); transfer(bob, alice);
  assert.deepEqual(alice.get('title'), ['overwritten']);
  alice.setWritePermissions(new Map([[alice.author, 'unbounded']]));
  assert.deepEqual(alice.get('title'), ['Hello']);
  alice.setWritePermissions(permissions);
  assert.deepEqual(alice.get('title'), ['overwritten']);
});

test('drafts are visible locally but not publishable before external signing', t => {
  const doc = make(t);
  const hash = doc.prepareSet('title', 'draft');
  assert.deepEqual(doc.get('title'), ['draft']);
  assert.equal(doc.graph.hasCommit(hash), false);
  assert.equal(doc.graph.exportChanges(new ExportCursor()).added.length, 0);
  assert.deepEqual(doc.getAt(doc.graph.signedViewPoint(), 'title'), []);
  assert.deepEqual(doc.get('title'), ['draft']);
  doc.publish();
  assert.equal(doc.graph.hasCommit(hash), true);
  assert.ok(doc.graph.exportChanges(new ExportCursor()).added.length);
});

test('prepareSet retains and materializes a draft before reporting deferred rejection', t => {
  const doc = make(t);
  const commit = doc.graph.commit.bind(doc.graph);
  const rejection = new StockfoilError('InvalidFragment', 'synthetic queued rejection');
  let hash;
  doc.graph.commit = (...args) => {
    const draft = commit(...args);
    hash = draft.hash;
    return { ...draft, report: { ...draft.report, rejected: [
      ...draft.report.rejected, { item: { kind: 'commit', hash }, error: rejection },
    ] } };
  };
  assert.throws(() => doc.prepareSet('title', 'draft'), error => {
    assert.ok(error instanceof AggregateError);
    assert.deepEqual(error.errors, [rejection]);
    return true;
  });
  assert.deepEqual(doc.get('title'), ['draft']);
  assert.equal(doc.pending.has(hash), true);
  assert.equal(doc.graph.hasCommit(hash), false);
  doc.publish();
  assert.equal(doc.graph.hasCommit(hash), true);
  assert.equal(doc.pending.size, 0);
});

test('publish materializes and removes signed drafts before reporting deferred rejection', t => {
  const doc = make(t);
  const first = doc.prepareSet('first', 'one');
  const second = doc.prepareSet('second', 'two');
  const addSignature = doc.graph.addSignature.bind(doc.graph);
  const rejection = new StockfoilError('InvalidFragment', 'synthetic queued rejection');
  const attempts = [];
  doc.graph.addSignature = evidence => {
    attempts.push(evidence.commit);
    const report = addSignature(evidence);
    return attempts.length === 1 ? { ...report, rejected: [
      ...report.rejected, { item: { kind: 'commit', hash: first }, error: rejection },
    ] } : report;
  };
  const update = doc.update.bind(doc);
  let updates = 0;
  doc.update = () => { updates++; update(); };
  assert.throws(() => doc.publish(), error => {
    assert.ok(error instanceof AggregateError);
    assert.deepEqual(error.errors, [rejection]);
    return true;
  });
  assert.equal(updates, 1);
  assert.equal(doc.graph.hasCommit(first), true);
  assert.equal(doc.graph.hasCommit(second), false);
  assert.deepEqual([...doc.pending.keys()], [second]);
  assert.deepEqual(doc.get('first'), ['one']);
  assert.deepEqual(doc.getAt(doc.graph.signedViewPoint(), 'first'), ['one']);
  doc.publish();
  assert.equal(updates, 2);
  assert.deepEqual(attempts, [first, second]);
  assert.equal(doc.pending.size, 0);
  assert.equal(doc.graph.hasCommit(second), true);
  assert.deepEqual(doc.get('second'), ['two']);
});

test('publish failure preserves unsigned drafts and materializes earlier successful signatures', t => {
  const doc = make(t);
  const first = doc.prepareSet('first', 'one');
  const second = doc.prepareSet('second', 'two');
  const third = doc.prepareSet('third', 'three');
  const addSignature = doc.graph.addSignature.bind(doc.graph);
  const failure = new Error('signing failed');
  doc.graph.addSignature = evidence => {
    if (evidence.commit === second) throw failure;
    return addSignature(evidence);
  };
  const update = doc.update.bind(doc);
  let updates = 0;
  doc.update = () => { updates++; update(); };
  assert.throws(() => doc.publish(), error => error === failure);
  assert.equal(updates, 1);
  assert.equal(doc.graph.hasCommit(first), true);
  assert.equal(doc.graph.hasCommit(second), false);
  assert.equal(doc.graph.hasCommit(third), false);
  assert.deepEqual([...doc.pending.keys()], [second, third]);
  assert.deepEqual(doc.getAt(doc.graph.signedViewPoint(), 'first'), ['one']);
  doc.graph.addSignature = addSignature;
  doc.publish();
  assert.equal(updates, 2);
  assert.equal(doc.pending.size, 0);
  assert.equal(doc.graph.hasCommit(third), true);
  assert.deepEqual(doc.get('third'), ['three']);
});

test('receiving signed history never grants write permission', t => {
  const source = make(t), target = make(t);
  source.set('key', 'value'); transfer(source, target);
  assert.deepEqual(target.get('key'), []);
  target.setWritePermissions(new Map([[source.author, 'unbounded']]));
  assert.deepEqual(target.get('key'), ['value']);
});

test('batch failure still materializes earlier valid encoded items', t => {
  const source = make(t), target = make(t);
  const hash = source.set('title', 'valid');
  target.setWritePermissions(new Map([[source.author, 'unbounded']]));
  const { bytes } = source.graph.exportChanges(new ExportCursor()).added[0];
  assert.throws(() => target.ingest([bytes, Uint8Array.of(0)]));
  assert.equal(target.graph.hasCommit(hash), true);
  assert.deepEqual(target.get('title'), ['valid']);
});

test('private BCS assignment encoding matches Rust strings exactly', () => {
  assert.equal(Buffer.from(encodeAssignment({ key: 'title', value: 'Hello' })).toString('hex'), '057469746c650548656c6c6f');
  const assignment = { key: '\uFEFF🔑', value: 'é'.repeat(100) };
  assert.deepEqual(decodeAssignment(encodeAssignment(assignment)), assignment);
  assert.throws(() => decodeAssignment(Uint8Array.of(0x80, 0, 0))); // noncanonical ULEB128
  assert.throws(() => decodeAssignment(Uint8Array.of(1, 0xff, 0))); // invalid UTF-8
  assert.throws(() => decodeAssignment(Uint8Array.of(0, 0, 0))); // trailing data
});

test('encoded roundtrips, truncation, framing, trailing bytes, signature/payload forgery', t => {
  const source = make(t), target = make(t);
  const hash = source.set('title', 'hello');
  const encoded = source.graph.exportChanges(new ExportCursor()).added[0].bytes;
  target.setWritePermissions(new Map([[source.author, 'unbounded']]));
  for (let n = 0; n < encoded.length; n++) assert.throws(() => target.ingest([encoded.slice(0, n)]));
  assert.throws(() => target.ingest([Uint8Array.from([...encoded, 0])]));
  const framing = encoded.slice(); framing[0] ^= 0xff;
  assert.throws(() => target.ingest([framing]));
  for (const forged of forgeries(source, hash, { key: 'title', value: 'hello' })) {
    assert.throws(() => target.ingest([forged]));
    assert.deepEqual(target.get('title'), []);
    assert.equal(target.graph.hasCommit(hash), false);
  }
  target.ingest([encoded]);
  assert.deepEqual(target.get('title'), ['hello']);
});

test('fragment BCS fixed-LE key runs, dictionary order and pre-expansion limits', () => {
  const shape = { members: [{}, {}] };
  const payloads = [{ key: 'k', value: 'a' }, { key: 'k', value: 'b' }];
  const bytes = registerCodec.encodeFragment(shape, payloads);
  assert.equal(Buffer.from(bytes).toString('hex'), '01016b0100000000020000000201610162');
  assert.deepEqual(registerCodec.decodeFragment(shape, bytes), payloads);
  for (let n = 0; n < bytes.length; n++) assert.throws(() => registerCodec.decodeFragment(shape, bytes.slice(0, n)));
  for (const [offset, value] of [[8, 0xffffffff], [8, 0], [4, 1]]) {
    const malformed = bytes.slice(); new DataView(malformed.buffer).setUint32(offset, value, true);
    assert.throws(() => registerCodec.decodeFragment(shape, malformed));
  }
  assert.throws(() => registerCodec.decodeFragment(shape, Uint8Array.from([...bytes, 0])));
  const limits = { maxMembers: 2, maxItemBytes: 1024, maxPayloadBytes: 7 };
  assert.throws(() => registerCodec.decodeFragment(shape, bytes, limits));
  const mixed = [{ key: 'z', value: '1' }, { key: 'a', value: '2' }, { key: 'z', value: '3' }];
  const mixedShape = { members: [{}, {}, {}] };
  const mixedBytes = registerCodec.encodeFragment(mixedShape, mixed);
  assert.equal(Buffer.from(mixedBytes).toString('hex'), '02017a01610300000000010000000100000001000000000000000100000003013101320133');
  assert.deepEqual(registerCodec.decodeFragment(mixedShape, mixedBytes), mixed);
});
