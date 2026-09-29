import test from 'node:test';
import assert from 'node:assert/strict';
import { ExportCursor } from 'stockfoil';
import { MvRegisterMap } from '../register.mjs';
import { encode, decode, encodeAssignment, decodeAssignment } from '../wire.mjs';

function make(t) { const doc = new MvRegisterMap(); t.after(() => doc.dispose()); return doc; }
function transfer(from, to) {
  const items = from.graph.exportChanges(new ExportCursor()).added;
  to.ingest([...items].reverse().map(item => decode(encode(item))));
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

test('receiving signed history never grants write permission', t => {
  const source = make(t), target = make(t);
  source.set('key', 'value'); transfer(source, target);
  assert.deepEqual(target.get('key'), []);
  target.setWritePermissions(new Map([[source.author, 'unbounded']]));
  assert.deepEqual(target.get('key'), ['value']);
});

test('private BCS assignment encoding matches Rust strings exactly', () => {
  assert.equal(Buffer.from(encodeAssignment({ key: 'title', value: 'Hello' })).toString('hex'), '057469746c650548656c6c6f');
  const assignment = { key: '\uFEFF🔑', value: 'é'.repeat(100) };
  assert.deepEqual(decodeAssignment(encodeAssignment(assignment)), assignment);
  assert.throws(() => decodeAssignment(Uint8Array.of(0x80, 0, 0))); // noncanonical ULEB128
  assert.throws(() => decodeAssignment(Uint8Array.of(1, 0xff, 0))); // invalid UTF-8
  assert.throws(() => decodeAssignment(Uint8Array.of(0, 0, 0))); // trailing data
});

test('wire roundtrips, truncation, version, trailing bytes, signature/payload forgery', t => {
  const source = make(t); source.set('title', 'hello');
  const item = source.graph.exportChanges(new ExportCursor()).added[0];
  const encoded = encode(item);
  assert.deepEqual(decode(encoded), item);
  for (let n = 0; n < encoded.length; n++) assert.throws(() => decode(encoded.slice(0, n)));
  assert.throws(() => decode(Uint8Array.from([...encoded, 0])));
  const version = encoded.slice(); version[0] = 2;
  assert.throws(() => decode(version));
  for (const tamper of [
    item => { item.signatures[0].signature[0] ^= 1; },
    item => { (item.kind === 'commit' ? item.commit : item.commits[0]).payload[2] ^= 1; },
  ]) {
    const target = make(t), forged = structuredClone(item);
    target.setWritePermissions(new Map([[source.author, 'unbounded']]));
    tamper(forged);
    assert.throws(() => target.ingest([decode(encode(forged))]));
    assert.deepEqual(target.get('title'), []);
  }
});
