import test from 'node:test';
import assert from 'node:assert/strict';
import { CommitGraph, MaterializationCursor, ExportCursor, StockfoilError, describeCommit } from 'stockfoil';
import { CommitGraph as SlimGraph, MaterializationCursor as SlimCursor } from 'stockfoil/slim';
import { alice, snapshot, signer } from './support.mjs';
import { readFileSync } from 'node:fs';
import { runsCodec } from './runs-codec.mjs';

const utf8 = new TextEncoder();
const text = new TextDecoder('utf-8', { fatal: true });
const encode = value => utf8.encode(JSON.stringify(value));
const decode = bytes => JSON.parse(text.decode(bytes));
function codec(overrides = {}) {
  return {
    formatId: 'stockfoil/test-json/v1',
    encodeCommit: encode,
    decodeCommit: decode,
    encodeFragment(_shape, values) { return encode(values); },
    decodeFragment(_shape, bytes) { return decode(bytes); },
    ...overrides,
  };
}
function graph(t, c = codec(), limits) {
  const result = new CommitGraph(c, limits);
  t.after(() => result.dispose());
  result.setWritePermissions(new Map([[alice.author, 'unbounded']]));
  return result;
}
function mine(g, depth, key = 'title') {
  for (let nonce = 0; nonce < 1_000_000; nonce++) {
    const payload = { key, value: `value ${nonce}` };
    const meta = g.describeCommit(alice.author, payload);
    if (meta.hash.startsWith('00'.repeat(depth)) && meta.hash.slice(2 * depth, 2 * depth + 2) !== '00') return payload;
  }
  throw new Error('mining failed');
}
function signed(g, payload) {
  const draft = g.commit(alice.author, payload);
  g.addSignature(alice.proof(draft.hash));
  return draft;
}
function materialized(g) {
  return g.changes(new MaterializationCursor()).changes.map(c => c.commit.payload);
}
function unchanged(g, operation, code, cause) {
  const before = snapshot(g);
  assert.throws(operation, error => {
    assert.ok(error instanceof StockfoilError);
    assert.equal(error.code, code);
    if (cause !== undefined) assert.equal(error.cause, cause);
    return true;
  });
  assert.deepEqual(snapshot(g), before);
}

test('codec is required and its complete synchronous interface and budgets are validated', () => {
  for (const invalid of [undefined, {}, codec({ encodeFragment: undefined }), codec({ formatId: '' }), codec({ formatId: 'é'.repeat(65) })]) {
    assert.throws(() => new CommitGraph(invalid), { code: 'InvalidInput' });
  }
  for (const maxMembers of [-1, NaN, Infinity, 1.5, 2 ** 32, undefined]) {
    assert.throws(() => new CommitGraph(codec(), { maxMembers }), { code: 'InvalidInput' });
  }
});

test('typed standalone roundtrip preserves native hash identity, cursors and owned exports', t => {
  const source = graph(t), target = graph(t);
  const payload = mine(source, 0);
  const prepared = source.describeCommit(alice.author, payload);
  assert.equal(prepared.hash, describeCommit(alice.author, [], encode(payload)).hash);
  assert.deepEqual(materialized(source), []);
  const draft = source.commit(alice.author, payload);
  assert.equal(draft.hash, prepared.hash);
  assert.equal(source.hasCommit(draft.hash), false);
  assert.deepEqual(source.exportChanges(new ExportCursor()).added, []);
  source.addSignature(alice.proof(draft.hash));
  const batch = source.exportChanges(new ExportCursor());
  assert.equal(batch.added.length, 1);
  const exported = batch.added[0];
  assert.deepEqual(exported.metadata, { kind: 'commit', hash: draft.hash, parents: [] });
  target.ingest(exported.bytes);
  assert.equal(target.hasCommit(draft.hash), true);
  assert.deepEqual(materialized(target), [payload]);
  assert.deepEqual(source.changes(target.changes(new SlimCursor()).cursor.clone()).changes, []);
  assert.deepEqual(source.exportChanges(batch.cursor.clone()).added, []);
  const saved = structuredClone(exported);
  exported.bytes.fill(0);
  exported.metadata.parents.push(draft.hash);
  assert.deepEqual(source.exportChanges(new ExportCursor()).added[0], saved);
  source.dispose();
  assert.equal(saved.bytes[0] === 0, false);
  assert.throws(() => source.pendingItems(), { code: 'Disposed' });
  source.dispose();
});

test('fragment callbacks see aligned topology; reverse catchup uses the native pending queue', t => {
  const shapes = [];
  let fragmentCalls = 0;
  const c = codec({
    encodeFragment(shape, values) {
      fragmentCalls++;
      assert.equal(shape.members.length, values.length);
      shapes.push(structuredClone(shape));
      // Application-specific column representation, not a Stockfoil envelope.
      return encode({ keys: values.map(p => p.key), values: values.map(p => p.value) });
    },
    decodeFragment(shape, bytes, budget) {
      assert.ok(budget.maxMembers >= shape.members.length);
      const columns = decode(bytes);
      return columns.keys.map((key, i) => ({ key, value: columns.values[i] }));
    },
  });
  const source = graph(t, c), target = graph(t, c);
  signed(source, mine(source, 1, 'boundary'));
  signed(source, mine(source, 0));
  signed(source, mine(source, 0));
  const head = signed(source, mine(source, 1));
  const batch = source.exportChanges(new ExportCursor());
  const fragments = batch.added.filter(i => i.metadata.kind === 'fragment');
  assert.ok(fragments.length >= 2);
  assert.ok(fragmentCalls > 0);
  for (const shape of shapes) {
    for (const [index, member] of shape.members.entries()) {
      assert.equal(member.author, alice.author);
      for (const parent of member.parents) {
        if (parent.kind === 'member') assert.ok(parent.index < index);
        else assert.match(parent.hash, /^[0-9a-f]{64}$/);
      }
    }
  }
  const last = fragments.find(i => i.metadata.head === head.hash);
  assert.ok(last.metadata.boundary.length > 0);
  assert.equal(target.ingest(last.bytes).pending, 1);
  assert.equal(target.hasCommit(head.hash), false);
  for (const item of batch.added.filter(i => i !== last).reverse()) target.ingest(item.bytes);
  assert.equal(target.pendingItems(), 0);
  assert.equal(target.hasCommit(head.hash), true);
  assert.deepEqual(materialized(target), materialized(source));
});

test('inputs, encoder arguments, decoder results and materialization never retain caller aliases', t => {
  let encoderArgument, decodedArgument;
  const c = codec({
    encodeCommit(value) { encoderArgument = value; return encode(value); },
    decodeCommit(bytes) { decodedArgument = decode(bytes); return decodedArgument; },
  });
  const source = graph(t, c), target = graph(t, c);
  const payload = { nested: { text: 'original' }, bytes: [1, 2] };
  const expected = structuredClone(payload);
  const draft = signed(source, payload);
  payload.nested.text = 'mutated';
  encoderArgument.nested.text = 'mutated callback alias';
  assert.deepEqual(materialized(source), [expected]);
  const exported = source.exportChanges(new ExportCursor()).added[0];
  encoderArgument.bytes.push(3);
  target.ingest(exported.bytes);
  decodedArgument.nested.text = 'mutated decoder alias';
  encoderArgument.nested.text = 'mutated reencoder alias';
  const delivered = materialized(target);
  delivered[0].nested.text = 'mutated materialization';
  assert.deepEqual(materialized(source), [expected]);
  assert.deepEqual(materialized(target), [expected]);
});

test('fragment encoder and decoder batches are isolated copies', t => {
  let encodedValues, decodedValues;
  const c = codec({
    encodeFragment(_shape, values) { encodedValues = values; return encode(values); },
    decodeFragment(_shape, bytes) { decodedValues = decode(bytes); return decodedValues; },
  });
  const source = graph(t, c), target = graph(t, c);
  signed(source, mine(source, 0));
  signed(source, mine(source, 1));
  const expected = materialized(source);
  const fragment = source.exportChanges(new ExportCursor()).added.find(i => i.metadata.kind === 'fragment');
  encodedValues[0].value = 'encoder alias';
  assert.deepEqual(materialized(source), expected);
  target.ingest(fragment.bytes);
  decodedValues[0].value = 'decoder alias';
  decodedValues.reverse();
  assert.deepEqual(materialized(target), expected);
  materialized(target)[0].value = 'reader alias';
  assert.deepEqual(materialized(target), expected);
});

test('mutation inside an encoder cannot mutate retained payloads; changed canonical output is rejected', t => {
  let mutate = false;
  const source = graph(t, codec({ encodeCommit(value) {
    if (mutate) value.value = 'changed';
    return encode(value);
  } }));
  const payload = mine(source, 0);
  const draft = signed(source, payload);
  mutate = true;
  assert.throws(() => source.exportChanges(new ExportCursor()), { code: 'PayloadHashMismatch' });
  assert.deepEqual(materialized(source), [payload]);
  mutate = false;
  assert.ok(source.exportChanges(new ExportCursor()).added[0].bytes.length);
});

test('codec exceptions preserve exact causes and local/decode failures are atomic', t => {
  const failure = new Error('application failed');
  let fail = true;
  const source = graph(t, codec({ encodeCommit(value) { if (fail) throw failure; return encode(value); } }));
  unchanged(source, () => source.commit(alice.author, { value: 'x' }), 'CodecError', failure);
  fail = false;
  const draft = signed(source, mine(source, 0));
  const item = source.exportChanges(new ExportCursor()).added[0];
  const target = graph(t, codec({ decodeCommit() { throw failure; } }));
  unchanged(target, () => target.ingest(item.bytes), 'CodecError', failure);
  const thrownValue = { custom: 'not an Error' };
  const other = graph(t, codec({ encodeCommit() { throw thrownValue; } }));
  unchanged(other, () => other.commit(alice.author, {}), 'CodecError', thrownValue);
});

test('Promise, thenable and non-byte encoder outputs are rejected before mutation', t => {
  for (const result of [Promise.resolve(new Uint8Array()), { then() {} }, [], new ArrayBuffer(1), new Uint16Array(1), 'bytes', null]) {
    const g = graph(t, codec({ encodeCommit() { return result; } }));
    unchanged(g, () => g.commit(alice.author, {}), 'CodecError');
  }
  const source = graph(t);
  const draft = signed(source, mine(source, 0));
  const item = source.exportChanges(new ExportCursor()).added[0];
  for (const result of [Promise.resolve({}), { then() {} }]) {
    const g = graph(t, codec({ decodeCommit() { return result; } }));
    unchanged(g, () => g.ingest(item.bytes), 'CodecError');
  }
});

test('synchronous reentry, including reads and disposal, is rejected without a Wasm panic', t => {
  let g;
  let operations = [];
  let invoked = 0;
  const c = codec({ encodeCommit(value) {
    for (const operation of operations) { assert.throws(operation, { code: 'ReentrantCall' }); invoked++; }
    return encode(value);
  } });
  g = graph(t, c);
  operations = [
    () => g.commit(alice.author, {}), () => g.ingest(new Uint8Array()),
    () => g.viewPoint(), () => g.pendingItems(), () => g.dispose(),
    () => g.changes(new MaterializationCursor()), () => g.exportChanges(new ExportCursor()),
    () => g.describeCommit(alice.author, {}), () => g.setWritePermissions(new Map()),
    () => g.setDecodeLimits({}),
  ];
  signed(g, { value: 'safe' });
  g.exportChanges(new ExportCursor());
  assert.ok(invoked >= operations.length * 2);
  assert.deepEqual(materialized(g), [{ value: 'safe' }]);
  const reentrant = graph(t, codec({ encodeCommit() { return reentrant.viewPoint(); } }));
  assert.throws(() => reentrant.commit(alice.author, {}), error => {
    assert.equal(error.code, 'CodecError');
    assert.equal(error.cause.code, 'ReentrantCall');
    return true;
  });
  assert.deepEqual(materialized(reentrant), []);
});

test('wrong format, malformed envelopes, limits and noncanonical decoding fail atomically', t => {
  const source = graph(t);
  const draft = signed(source, mine(source, 0));
  const item = source.exportChanges(new ExportCursor()).added[0];
  let called = false;
  const wrong = graph(t, codec({ formatId: 'different/v1', decodeCommit() { called = true; return {}; } }));
  unchanged(wrong, () => wrong.ingest(item.bytes), 'WrongCodec');
  assert.equal(called, false);
  const small = graph(t, codec(), { maxItemBytes: item.bytes.length - 1 });
  unchanged(small, () => small.ingest(item.bytes), 'LimitExceeded');
  const noPayload = graph(t, codec(), { maxPayloadBytes: 0 });
  unchanged(noPayload, () => noPayload.ingest(item.bytes), 'LimitExceeded');
  const noSignatures = graph(t, codec(), { maxSignatures: 0 });
  unchanged(noSignatures, () => noSignatures.ingest(item.bytes), 'LimitExceeded');
  const invalid = graph(t);
  for (const bytes of [new Uint8Array(), item.bytes.slice(0, -1), new Uint8Array([...item.bytes, 0])]) {
    unchanged(invalid, () => invalid.ingest(bytes), 'InvalidEncoding');
  }
  const noncanonical = graph(t, codec({ decodeCommit() { return { changed: true }; } }));
  unchanged(noncanonical, () => noncanonical.ingest(item.bytes), 'NonCanonicalPayload');
});

test('failed fragment encoding does not advance export cursors; malformed fragment callback results are atomic', t => {
  const failure = new Error('fragment encode failed');
  let fail = false;
  const source = graph(t, codec({ encodeFragment(_shape, payloads) {
    if (fail) throw failure;
    return encode(payloads);
  } }));
  signed(source, mine(source, 0));
  signed(source, mine(source, 1));
  const cursor = new ExportCursor();
  fail = true;
  assert.throws(() => source.exportChanges(cursor), e => e.code === 'CodecError' && e.cause === failure);
  fail = false;
  const batch = source.exportChanges(cursor);
  const fragment = batch.added.find(i => i.metadata.kind === 'fragment');
  assert.ok(fragment);
  assert.deepEqual(source.exportChanges(cursor).added, batch.added);
  assert.deepEqual(source.exportChanges(batch.cursor).added, []);
  for (const result of [[], {}, Promise.resolve([])]) {
    const target = graph(t, codec({ decodeFragment() { return result; } }));
    unchanged(target, () => target.ingest(fragment.bytes), 'CodecError');
  }
  for (const result of [Promise.resolve(new Uint8Array()), [], new Uint16Array(1)]) {
    const bad = graph(t, codec({ encodeFragment() { return result; } }));
    for (const item of batch.added) bad.ingest(item.bytes);
    assert.throws(() => bad.exportChanges(new ExportCursor()), { code: 'CodecError' });
    assert.deepEqual(materialized(bad), materialized(source));
  }
  const small = graph(t, codec(), { maxMembers: 1 });
  unchanged(small, () => small.ingest(fragment.bytes), 'LimitExceeded');
  const noParents = graph(t, codec(), { maxParents: 0 });
  unchanged(noParents, () => noParents.ingest(fragment.bytes), 'LimitExceeded');
  small.setDecodeLimits({ maxMembers: 100 });
  noParents.setDecodeLimits({ maxParents: 100 });
  assert.deepEqual(materialized(small), []);
  assert.deepEqual(materialized(noParents), []);
  small.ingest(fragment.bytes);
  noParents.ingest(fragment.bytes);
  assert.deepEqual(materialized(small), materialized(source));
  assert.deepEqual(materialized(noParents), materialized(source));
});

test('budgets reach synchronous decoders, root/slim typed graphs share cursors, shared memory is forbidden', t => {
  let received;
  const source = graph(t);
  const draft = signed(source, mine(source, 0));
  const target = new SlimGraph(codec({ decodeCommit(bytes, limits) { received = limits; return decode(bytes); } }), { maxMembers: 17 });
  t.after(() => target.dispose());
  target.ingest(source.exportChanges(new ExportCursor()).added[0].bytes);
  assert.equal(received.maxMembers, 17);
  assert.equal(received.maxItemBytes, 8 * 1024 * 1024);
  assert.equal(received.maxParents, 1_000_000);
  assert.equal(target.changes(new MaterializationCursor()).changes.length, 1);
  assert.deepEqual(source.changes(source.changes(new SlimCursor()).cursor).changes, []);
  if (typeof SharedArrayBuffer !== 'undefined') {
    for (const value of [new SharedArrayBuffer(8), { nested: new Uint8Array(new SharedArrayBuffer(8)) }, new Map([['x', new SharedArrayBuffer(8)]]), new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true })]) {
      unchanged(source, () => source.commit(alice.author, value), 'CodecError');
    }
  }
  unchanged(source, () => source.commit(alice.author, { callback() {} }), 'CodecError');
});

test('invalid viewpoints consistently throw InvalidInput without changing the graph', t => {
  const g = graph(t);
  signed(g, { value: 'retained' });
  for (const view of [null, 42, 'view', { get writePermissions() { throw new Error('getter'); } }]) {
    unchanged(g, () => g.changes(new MaterializationCursor(), view), 'InvalidInput');
  }
});

test('inbound limits never restrict local creation/export; replacement and explicit retry update both layers', t => {
  const source = graph(t, codec(), { maxItemBytes: 0, maxPayloadBytes: 0, maxMembers: 0, maxParents: 0, maxSignatures: 0 });
  const prepared = source.describeCommit(alice.author, { value: 'local' });
  const draft = signed(source, { value: 'local' });
  assert.equal(prepared.hash, draft.hash);
  const item = source.exportChanges(new ExportCursor()).added[0];
  assert.ok(item.bytes.length);
  let received;
  const target = graph(t, codec({ decodeCommit(bytes, budget) { received = budget; return decode(bytes); } }), { maxItemBytes: 0, maxPayloadBytes: 0 });
  unchanged(target, () => target.ingest(item.bytes), 'LimitExceeded');
  target.setDecodeLimits({ maxItemBytes: item.bytes.length, maxPayloadBytes: 0 });
  unchanged(target, () => target.ingest(item.bytes), 'LimitExceeded');
  assert.equal(target.pendingItems(), 0);
  // Invalid replacement is atomic, including the JS pre-copy item limit.
  assert.throws(() => target.setDecodeLimits({ maxItemBytes: 0, maxPayloadBytes: -1 }), { code: 'InvalidInput' });
  unchanged(target, () => target.ingest(item.bytes), 'LimitExceeded');
  target.setDecodeLimits({ maxPayloadBytes: 1024 }); // Resets maxItemBytes to its default.
  assert.equal(target.hasCommit(draft.hash), false); // No implicit retry.
  target.ingest(item.bytes);
  assert.equal(target.hasCommit(draft.hash), true);
  assert.equal(received.maxItemBytes, 8 * 1024 * 1024);
  assert.equal(received.maxPayloadBytes, 1024);
  target.setDecodeLimits({ maxItemBytes: item.bytes.length - 1 });
  unchanged(target, () => target.ingest(item.bytes), 'LimitExceeded');
  assert.throws(() => target.setDecodeLimits({ maxItemBytes: item.bytes.length, maxMembers: NaN }), { code: 'InvalidInput' });
  unchanged(target, () => target.ingest(item.bytes), 'LimitExceeded');
  target.setDecodeLimits({});
  target.ingest(item.bytes);
  assert.equal(received.maxPayloadBytes, 8 * 1024 * 1024);
  target.dispose();
  assert.throws(() => target.setDecodeLimits({}), { code: 'Disposed' });
});

test('complete native/Wasm singleton and fragment golden envelopes match bytes, hashes and payloads', t => {
  const who = signer(7); // Same deterministic Ed25519 seed as tests/codec.rs.
  for (const kind of ['commit', 'fragment']) {
    const fixture = new Uint8Array(Buffer.from(readFileSync(new URL(`../../tests/fixtures/codec-${kind}-v1.hex`, import.meta.url), 'utf8').trim(), 'hex'));
    const source = graph(t, runsCodec(), { maxItemBytes: 0, maxPayloadBytes: 0 });
    const target = graph(t, runsCodec());
    const payloads = kind === 'commit' ? ['hello'] : [
      ...Array(100).fill('shared long application key and repeated value'), 'checkpoint:129955',
    ];
    let parents = [], last;
    const expected = [];
    for (const payload of payloads) {
      const meta = describeCommit(who.author, parents, utf8.encode(payload));
      last = source.commit(who.author, payload);
      assert.equal(last.hash, meta.hash);
      expected.push({ hash: meta.hash, payload });
      parents = [meta.hash];
    }
    source.addSignature(who.proof(last.hash));
    const batch = source.exportChanges(new ExportCursor());
    assert.equal(batch.added.length, 1);
    const item = batch.added[0];
    assert.equal(item.metadata.kind, kind);
    assert.deepEqual(item.bytes, fixture);
    target.ingest(fixture);
    const actual = target.changes(new MaterializationCursor()).changes.map(({ commit }) => ({ hash: commit.hash, payload: commit.payload }));
    assert.deepEqual(actual, expected);
    assert.deepEqual(target.exportChanges(new ExportCursor()).added, batch.added);
    assert.deepEqual(target.exportChanges(new ExportCursor()).added[0].bytes, fixture);
    assert.equal(target.hasCommit(last.hash), true);
  }
});
