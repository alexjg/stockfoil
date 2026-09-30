import assert from 'node:assert/strict';
import test from 'node:test';
import { authorId, commitHash, describeCommit, ExportCursor, MaterializationCursor } from 'stockfoil';
import {
  alice, bob, carol, unknown, bytes, copy, atDepth, signedCommit, fragment,
  itemId, assertIds, assertReport, assertErrorCode, throwsCode, graph, events,
  assertTopological, rejectsWithoutMutation,
} from './support.mjs';
import { encodeFixture } from './fixture-codec.mjs';

test('local drafts deliver immediately; signing authenticates without redelivery', t => {
  const g = graph(t, [alice]);
  const payload = bytes('local edit');
  const expected = describeCommit(alice.author, [], payload);
  const draft = g.commit(alice.author, payload);
  assert.equal(draft.hash, expected.hash);
  assert.equal(commitHash(draft.signingBytes), draft.hash);
  assert.equal(draft.signingBytes.length, 32);
  assertReport(draft.report);
  assert.equal(g.hasCommit(draft.hash), false);
  assert.equal(g.pendingItems(), 0);
  assert.deepEqual(g.missingDependencies(), []);
  assert.deepEqual(g.viewPoint().heads, [draft.hash]);
  assert.deepEqual(g.signedViewPoint().heads, []);
  assert.deepEqual(g.exportChanges(new ExportCursor()).added, []);
  const signed = g.changes(new MaterializationCursor(), g.signedViewPoint());
  const working = g.changes(new MaterializationCursor());
  assert.deepEqual(events(signed), [[draft.hash, true, 'neither']]);
  assert.deepEqual(events(working), [[draft.hash, true, 'after']]);
  const evidence = { author: alice.author, commit: draft.hash, signature: alice.signBytes(draft.signingBytes) };
  payload.fill(255);
  draft.signingBytes.fill(0);
  assert.deepEqual(working.changes[0].commit, expected);
  assertReport(g.addSignature(evidence), [{ kind: 'commit', hash: draft.hash }]);
  evidence.signature.fill(0);
  assert.equal(g.hasCommit(draft.hash), true);
  assert.deepEqual(g.signedViewPoint(), g.viewPoint());
  assert.deepEqual(g.changes(working.cursor).changes, []);
  assert.deepEqual(events(g.changes(signed.cursor, g.signedViewPoint())), [[draft.hash, false, 'after']]);
  const receiver = graph(t);
  for (const item of g.exportChanges(new ExportCursor()).added) receiver.ingest(item.bytes);
  assert.deepEqual(receiver.changes(new MaterializationCursor()).changes[0].commit, expected);
});

test('delivery includes denied authors; revocation changes participation, not authentication', t => {
  const g = graph(t, [alice]);
  const root = atDepth(alice), denied = atDepth(bob, [root.hash]), tip = atDepth(alice, [denied.hash]);
  for (const [commit, who] of [[root, alice], [denied, bob], [tip, alice]]) g.ingest(encodeFixture(signedCommit(commit, who)));
  const first = g.changes(new MaterializationCursor());
  assert.deepEqual(events(first), [[root.hash, true, 'after'], [denied.hash, true, 'neither'], [tip.hash, true, 'after']]);
  const published = g.exportChanges(new ExportCursor());
  assertIds(published.added.map(itemId), [root, denied, tip].map(c => ({ kind: 'commit', hash: c.hash })));
  assert.equal(g.hasCommit(denied.hash), true);
  g.setWritePermissions(new Map([[alice.author, { through: [root.hash] }], [bob.author, 'unbounded']]));
  const bounded = g.changes(first.cursor);
  assert.deepEqual(events(bounded), [[denied.hash, false, 'after'], [tip.hash, false, 'before']]);
  g.setWritePermissions(new Map([[alice.author, 'denied']]));
  const revoked = g.changes(bounded.cursor);
  assert.deepEqual(events(revoked), [[root.hash, false, 'before'], [denied.hash, false, 'before']]);
  assert.deepEqual(g.changes(revoked.cursor).changes, []);
  assert.deepEqual(g.exportChanges(published.cursor).added, []);
  assert.deepEqual(g.exportChanges(published.cursor).retired, []);
});

test('viewpoints, permissions, payloads and native envelopes are owned, even after disposal', t => {
  const g = graph(t), root = atDepth(alice);
  const incoming = encodeFixture(signedCommit(root));
  g.ingest(incoming);
  const permission = { through: [root.hash] }, permissions = new Map([[alice.author, permission]]);
  g.setWritePermissions(permissions);
  const saved = g.viewPoint(), expectedView = copy(saved);
  const batch = g.changes(new MaterializationCursor());
  const exported = g.exportChanges(new ExportCursor());
  const expectedItem = copy(exported.added[0]);
  permissions.clear(); permission.through.length = 0; incoming.fill(255);
  assert.deepEqual(g.viewPoint(), expectedView);
  assert.deepEqual(batch.changes[0].commit, root);
  saved.heads.length = 0; saved.writePermissions.get(alice.author).through.length = 0;
  batch.changes[0].commit.payload.fill(255); batch.changes[0].commit.parents.push(unknown);
  exported.added[0].bytes.fill(255); exported.added[0].metadata.parents.push(unknown);
  assert.deepEqual(g.viewPoint(), expectedView);
  assert.deepEqual(g.changes(new MaterializationCursor()).changes[0].commit, root);
  assert.deepEqual(g.exportChanges(new ExportCursor()).added[0], expectedItem);
  const retained = g.changes(new MaterializationCursor()), retainedExport = g.exportChanges(new ExportCursor());
  g.commit(alice.author, new Uint8Array(1024 * 1024));
  g.setWritePermissions(new Map()); g.dispose();
  assert.deepEqual(retained.changes[0].commit, root);
  assert.deepEqual(retainedExport.added[0], expectedItem);
  assertReport(graph(t).ingest(retainedExport.added[0].bytes), [itemId(expectedItem)]);
});

test('historical reads retain delivery and do not mutate the live cursor', t => {
  const g = graph(t, [alice]), root = g.commit(alice.author, bytes('root')), old = g.viewPoint();
  const child = g.commit(alice.author, bytes('child')), current = g.viewPoint();
  const past = g.changes(new MaterializationCursor(), old);
  assert.deepEqual(events(past), [[root.hash, true, 'after'], [child.hash, true, 'neither']]);
  const live = g.changes(past.cursor, current);
  assert.deepEqual(events(live), [[child.hash, false, 'after']]);
  const back = g.changes(live.cursor.clone(), old);
  assert.deepEqual(events(back), [[child.hash, false, 'before']]);
  assert.deepEqual(g.changes(live.cursor).changes, []);
  const later = g.commit(alice.author, bytes('later'));
  const stillPast = g.changes(back.cursor, old);
  assert.deepEqual(events(stillPast), [[later.hash, true, 'neither']]);
  assert.deepEqual(events(g.changes(stillPast.cursor)), [[child.hash, false, 'after'], [later.hash, false, 'after']]);
  assert.deepEqual(old.heads, [root.hash]);
});

test('cloned cursors work on equivalent graphs with different ingestion order', t => {
  const left = graph(t, [alice, bob]), right = graph(t);
  const a = atDepth(alice), b = atDepth(bob), join = atDepth(alice, [a.hash, b.hash]);
  left.ingest(encodeFixture(signedCommit(a))); left.ingest(encodeFixture(signedCommit(b, bob)));
  const cursor = left.changes(new MaterializationCursor()).cursor;
  left.ingest(encodeFixture(signedCommit(join)));
  for (const [c, who] of [[b, bob], [a, alice], [join, alice]]) right.ingest(encodeFixture(signedCommit(c, who)));
  const target = left.viewPoint(), expected = left.changes(cursor, target);
  assert.deepEqual(events(expected), [[join.hash, true, 'after']]);
  assert.deepEqual(right.changes(cursor.clone(), target).changes, expected.changes);
  assert.deepEqual(right.changes(expected.cursor, target).changes, []);
  assert.deepEqual(right.viewPoint().writePermissions, new Map());
  left.dispose();
  assert.deepEqual(events(right.changes(cursor.clone(), target)), events(expected));
});

test('unknown heads and missing cursor history fail without advancing cursors', t => {
  const g = graph(t, [alice]), initial = new MaterializationCursor(), draft = g.commit(alice.author, bytes('known'));
  for (const writePermissions of [new Map(), g.viewPoint().writePermissions]) {
    rejectsWithoutMutation(g, () => g.changes(initial, { heads: [unknown], writePermissions }), 'MissingCommit');
  }
  const batch = g.changes(initial, { heads: [], writePermissions: new Map() });
  assert.deepEqual(events(batch), [[draft.hash, true, 'neither']]);
  throwsCode(() => graph(t).changes(batch.cursor), 'MissingCommit');
  assert.deepEqual(g.changes(batch.cursor, { heads: [], writePermissions: new Map() }).changes, []);
  assert.deepEqual(events(g.changes(initial)), [[draft.hash, true, 'after']]);
});

test('reverse fan-in queues deduplicate, own bytes and drain into topological replayable batches', t => {
  const g = graph(t, [alice, bob]);
  const root = atDepth(alice), left = atDepth(alice, [root.hash]), right = atDepth(bob, [root.hash]);
  const join = atDepth(alice, [left.hash, right.hash, left.hash]), tip = atDepth(alice, [join.hash]);
  for (const [index, [commit, who]] of [[tip, alice], [join, alice], [right, bob], [left, alice]].entries()) {
    const input = encodeFixture(signedCommit(commit, who));
    assertReport(g.ingest(input), [], index + 1);
    assertReport(g.ingest(input.slice()), [], index + 1);
    assert.equal(g.hasCommit(commit.hash), false);
    input.fill(255);
  }
  assert.deepEqual(g.missingDependencies(), [root.hash, left.hash, right.hash, join.hash].sort());
  const missing = g.missingDependencies(); missing.length = 0;
  assert.equal(g.missingDependencies().length, 4);
  assert.deepEqual(g.viewPoint().heads, []);
  assert.deepEqual(g.changes(new MaterializationCursor()).changes, []);
  assert.deepEqual(g.exportChanges(new ExportCursor()).added, []);
  const commits = [root, left, right, join, tip];
  assertReport(g.ingest(encodeFixture(signedCommit(root))), commits.map(c => ({ kind: 'commit', hash: c.hash })));
  assert.deepEqual(g.missingDependencies(), []);
  const cursor = new MaterializationCursor(), target = g.viewPoint(), batch = g.changes(cursor);
  assertTopological(batch); assert.equal(batch.changes.length, commits.length);
  for (const commit of commits) assert.deepEqual(batch.changes.find(c => c.commit.hash === commit.hash).commit, commit);
  assert.equal(g.isAncestor(root.hash, join.hash), true); assert.equal(g.isAncestor(left.hash, right.hash), false);
  assert.throws(() => {
    for (const { commit } of batch.changes) {
      assert.equal(g.hasCommit(commit.hash), true);
      g.setWritePermissions(new Map());
      throw new Error('application failed');
    }
  }, /application failed/);
  assert.deepEqual(g.changes(cursor.clone(), target).changes, batch.changes);
  assert.deepEqual(g.changes(batch.cursor, target).changes, []);
  assertReport(g.ingest(encodeFixture(signedCommit(tip))), [{ kind: 'commit', hash: tip.hash }]);
  assert.deepEqual(g.changes(batch.cursor, target).changes, []);
});

test('export cursors support noop, retry, clone, independent destinations and increments', t => {
  const g = graph(t), empty = new ExportCursor(), first = g.exportChanges(empty);
  assert.deepEqual(first.added, []); assert.deepEqual(first.retired, []);
  const root = atDepth(alice), child = atDepth(alice, [root.hash]);
  for (const c of [root, child]) g.ingest(encodeFixture(signedCommit(c)));
  const full = g.exportChanges(first.cursor);
  assertIds(full.added.map(itemId), [root, child].map(c => ({ kind: 'commit', hash: c.hash })));
  assert.deepEqual(full.retired, []);
  for (const cursor of [empty, first.cursor, first.cursor.clone(), new ExportCursor()]) assert.deepEqual(g.exportChanges(cursor).added, full.added);
  assert.deepEqual(g.exportChanges(full.cursor).added, []);
  const tip = atDepth(bob, [child.hash]); g.ingest(encodeFixture(signedCommit(tip, bob)));
  const increment = g.exportChanges(full.cursor.clone());
  assertIds(increment.added.map(itemId), [{ kind: 'commit', hash: tip.hash }]);
  assert.deepEqual(increment.retired, []); assert.deepEqual(g.exportChanges(increment.cursor).added, []);
  const receiver = graph(t);
  for (const item of [...full.added, ...increment.added].reverse()) receiver.ingest(item.bytes);
  assert.equal(receiver.pendingItems(), 0); assert.deepEqual(receiver.signedViewPoint(), g.signedViewPoint());
});

test('checkpoint fragments retire loose certificates without rematerialization', t => {
  const g = graph(t, [alice]), root = atDepth(alice), head = atDepth(alice, [root.hash], 1);
  for (const c of [root, head]) g.ingest(encodeFixture(signedCommit(c)));
  const before = g.exportChanges(new ExportCursor()), materialized = g.changes(new MaterializationCursor()), view = g.viewPoint();
  const certificate = fragment(head, [head, root]);
  assertReport(g.ingest(encodeFixture(certificate)), [itemId(certificate)]);
  const change = g.exportChanges(before.cursor);
  assertIds(change.added.map(itemId), [itemId(certificate)]); assertIds(change.retired, before.added.map(itemId));
  assert.deepEqual(g.viewPoint(), view); assert.deepEqual(g.changes(materialized.cursor).changes, []);
  assert.deepEqual(g.exportChanges(change.cursor).added, []);
  assert.deepEqual(g.exportChanges(before.cursor.clone()).retired, change.retired);
  const receiver = graph(t);
  for (const item of [...before.added, ...change.added]) receiver.ingest(item.bytes);
  assert.equal(receiver.hasCommit(root.hash), true); assert.equal(receiver.hasCommit(head.hash), true);
});

test('deeper fragments retire earlier fragments; saved exports survive pruning', t => {
  const g = graph(t), root = atDepth(alice, [], 1), head = atDepth(alice, [root.hash], 2);
  const old = fragment(root, [root]), next = fragment(head, [head, root]);
  g.ingest(encodeFixture(old)); const saved = g.exportChanges(new ExportCursor());
  g.ingest(encodeFixture(next)); const change = g.exportChanges(saved.cursor);
  assertIds(change.added.map(itemId), [itemId(next)]); assertIds(change.retired, [itemId(old)]);
  assert.deepEqual(g.exportChanges(change.cursor).added, []); g.dispose();
  const receiver = graph(t);
  assertReport(receiver.ingest(saved.added[0].bytes), [itemId(old)]);
  assertReport(receiver.ingest(change.added[0].bytes), [itemId(next)]);
  assert.equal(receiver.hasCommit(root.hash), true); assert.equal(receiver.hasCommit(head.hash), true);
});

test('different sufficient evidence and permissions do not republish an identity', t => {
  const g = graph(t), root = atDepth(alice), head = atDepth(bob, [root.hash], 1);
  g.ingest(encodeFixture(fragment(head, [root, head], [], [alice.proof(root.hash), bob.proof(head.hash)])));
  const exported = g.exportChanges(new ExportCursor());
  g.ingest(encodeFixture(fragment(head, [head, root], [], [alice.proof(head.hash), bob.proof(head.hash)])));
  const fresh = g.exportChanges(new ExportCursor());
  assert.notDeepEqual(fresh.added[0].bytes, exported.added[0].bytes);
  g.setWritePermissions(new Map([[alice.author, 'unbounded'], [bob.author, 'unbounded']]));
  const noop = g.exportChanges(exported.cursor);
  assert.deepEqual(noop.added, []); assert.deepEqual(noop.retired, []);
  for (const item of [exported.added[0], fresh.added[0]]) assert.equal(graph(t).ingest(item.bytes).pending, 0);
});

test('signing a checkpoint wakes received dependants of promoted interior drafts', t => {
  const g = graph(t, [alice, bob]), root = atDepth(alice), middle = atDepth(alice, [root.hash]);
  const checkpoint = atDepth(alice, [middle.hash], 1), child = atDepth(bob, [middle.hash]), tip = atDepth(bob, [child.hash]);
  assertReport(g.ingest(encodeFixture(signedCommit(tip, bob))), [], 1);
  const incoming = encodeFixture(signedCommit(child, bob)); assertReport(g.ingest(incoming), [], 2); incoming.fill(255);
  for (const commit of [root, middle, checkpoint]) {
    const draft = g.commit(alice.author, commit.payload);
    assert.equal(draft.hash, commit.hash); assertReport(draft.report, [], 2); assert.equal(g.hasCommit(commit.hash), false);
  }
  assert.deepEqual(g.missingDependencies(), [middle.hash, child.hash].sort());
  assert.deepEqual(g.exportChanges(new ExportCursor()).added, []);
  const working = g.changes(new MaterializationCursor());
  assertReport(g.addSignature(alice.proof(checkpoint.hash)), [itemId(fragment(checkpoint, [])), itemId(signedCommit(child)), itemId(signedCommit(tip))]);
  assert.deepEqual(g.missingDependencies(), []);
  assert.deepEqual(events(g.changes(working.cursor)), [[child.hash, true, 'after'], [tip.hash, true, 'after']]);
  for (const c of [root, middle, checkpoint, child, tip]) assert.equal(g.hasCommit(c.hash), true);
  const receiver = graph(t);
  for (const item of g.exportChanges(new ExportCursor()).added.reverse()) receiver.ingest(item.bytes);
  assert.equal(receiver.pendingItems(), 0); assert.equal(receiver.hasCommit(tip.hash), true);
});

test('skipped export covers coalesce instead of reporting transient loose inventory', t => {
  const g = graph(t), root = atDepth(alice), middle = atDepth(alice, [root.hash]), head = atDepth(alice, [middle.hash], 1);
  g.ingest(encodeFixture(signedCommit(root))); const saved = g.exportChanges(new ExportCursor());
  g.ingest(encodeFixture(signedCommit(middle)));
  assert.equal(g.commit(alice.author, head.payload).hash, head.hash); g.addSignature(alice.proof(head.hash));
  const delta = g.exportChanges(saved.cursor);
  assertIds(delta.added.map(itemId), [itemId(fragment(head, []))]); assertIds(delta.retired, [itemId(signedCommit(root))]);
  assert.deepEqual(g.exportChanges(saved.cursor.clone()).added, delta.added); assert.deepEqual(g.exportChanges(delta.cursor).retired, []);
  const receiver = graph(t);
  for (const item of [...saved.added, ...delta.added]) receiver.ingest(item.bytes);
  assert.equal(receiver.hasCommit(middle.hash), true); assert.equal(receiver.hasCommit(head.hash), true);
});

test('metadata utility owns inputs; recomputing hashes cannot authenticate edits', t => {
  const g = graph(t), parents = [atDepth(bob).hash, atDepth(carol).hash], payload = bytes('original');
  const original = describeCommit(alice.author, parents, payload), saved = copy(original);
  parents.reverse(); payload.fill(255); assert.deepEqual(original, saved);
  original.payload.fill(0); original.parents.length = 0;
  assert.deepEqual(describeCommit(alice.author, saved.parents, bytes('original')), saved);
  for (const modified of [describeCommit(alice.author, saved.parents, bytes('tampered')), describeCommit(alice.author, [...saved.parents].reverse(), saved.payload), describeCommit(bob.author, saved.parents, saved.payload)]) {
    assert.notEqual(modified.hash, saved.hash);
    const evidence = { ...alice.proof(saved.hash), commit: modified.hash };
    rejectsWithoutMutation(g, () => g.ingest(encodeFixture({ kind: 'commit', commit: modified, signatures: [evidence] })), 'InvalidSignature');
  }
});

test('identifier, cursor and encoded input validation reports stable errors', t => {
  const g = graph(t);
  assert.equal(authorId(alice.publicKey), alice.author); assert.equal(commitHash(new Uint8Array(32).fill(171)), 'ab'.repeat(32));
  for (const convert of [authorId, commitHash]) for (const value of [new Uint8Array(31), new Uint8Array(33), [], '00']) throwsCode(() => convert(value), 'InvalidInput');
  for (const hash of ['AB'.repeat(32), 'g'.repeat(64), '0'.repeat(63), '0'.repeat(65), 123, null]) {
    rejectsWithoutMutation(g, () => g.hasCommit(hash), 'InvalidInput'); throwsCode(() => g.isAncestor(unknown, hash), 'InvalidInput');
  }
  for (const operation of [
    () => describeCommit(alice.author.toUpperCase(), [], bytes('x')),
    () => describeCommit(alice.author, [unknown.toUpperCase()], bytes('x')),
    () => g.setWritePermissions({ [alice.author]: 'unbounded' }),
    () => g.setWritePermissions(new Map([[alice.author, 'write']])),
    () => g.setWritePermissions(new Map([[alice.author, { through: ['bad'] }]])),
    () => g.changes(new MaterializationCursor(), { heads: [], writePermissions: {} }),
    () => g.changes(new ExportCursor()), () => g.exportChanges(new MaterializationCursor()), () => g.changes({}),
    () => g.ingest({ kind: 'commit' }), () => g.ingest([]),
  ]) rejectsWithoutMutation(g, operation, 'InvalidInput');
  rejectsWithoutMutation(g, () => g.commit(alice.author, [1, 2]), 'CodecError');
});

test('wire contents and every signature validate before mutation, including duplicates and queued items', t => {
  const root = atDepth(alice), other = atDepth(bob), child = atDepth(alice, [root.hash, other.hash, root.hash]);
  const original = signedCommit(child);
  // Wire v1 derives payload/member hashes; it does not carry redundant claimed
  // payloadHash fields. Changing contents contradicts the envelope head.
  const cases = [
    ['PayloadHashMismatch', i => { i.commit.payload[0] ^= 1; }],
    ['PayloadHashMismatch', i => { i.commit.hash = unknown; i.signatures[0].commit = unknown; }],
    ['PayloadHashMismatch', i => { i.commit.author = bob.author; }],
    ['PayloadHashMismatch', i => { i.commit.parents.reverse(); i.commit.parents.pop(); }],
    ['PayloadHashMismatch', i => { i.commit.parents = [other.hash, root.hash, root.hash]; }],
    ['MissingSignature', i => { i.signatures = []; }],
    ['MissingSignature', i => { i.signatures = [bob.proof(child.hash)]; }],
    ['InvalidSignature', i => { i.signatures[0].signature[0] ^= 1; }],
    ['InvalidSignature', i => { i.signatures[0].author = bob.author; }],
    ['InvalidEncoding', i => { i.signatures[0].signature = new Uint8Array(63); }],
    ['InvalidEncoding', i => { i.signatures = [alice.proof(root.hash)]; }],
    ['InvalidSignature', i => { const extra = bob.proof(child.hash); extra.signature[0] ^= 1; i.signatures.push(extra); }],
    ['InvalidPublicKey', i => { i.signatures.push({ author: '02'.repeat(32), commit: child.hash, signature: new Uint8Array(64) }); }],
  ];
  for (const state of ['fresh', 'queued', 'admitted']) {
    const g = graph(t);
    if (state === 'admitted') for (const [c, who] of [[root, alice], [other, bob]]) g.ingest(encodeFixture(signedCommit(c, who)));
    if (state !== 'fresh') g.ingest(encodeFixture(original));
    for (const [code, tamper] of cases) {
      const item = copy(original); tamper(item);
      const encoded = encodeFixture(item);
      rejectsWithoutMutation(g, () => g.ingest(encoded), code);
    }
    if (state !== 'admitted') for (const [c, who] of [[root, alice], [other, bob]]) g.ingest(encodeFixture(signedCommit(c, who)));
    assertReport(g.ingest(encodeFixture(original)), [itemId(original)]); assert.equal(g.hasCommit(child.hash), true);
  }
});

test('signature failures leave local drafts unsigned and nonexportable', t => {
  const g = graph(t, [alice]), draft = g.commit(alice.author, bytes('draft')), bad = alice.proof(draft.hash);
  bad.signature[0] ^= 1;
  rejectsWithoutMutation(g, () => g.addSignature(bad), 'InvalidSignature');
  rejectsWithoutMutation(g, () => g.addSignature({ ...bad, signature: new Uint8Array(65) }), 'InvalidInput');
  rejectsWithoutMutation(g, () => g.addSignature({ ...bad, author: '02'.repeat(32) }), 'InvalidPublicKey');
  rejectsWithoutMutation(g, () => g.addSignature(alice.proof(unknown)), 'NotPendingLocalCommit');
  assert.equal(g.hasCommit(draft.hash), false); assert.deepEqual(g.exportChanges(new ExportCursor()).added, []);
  g.addSignature(alice.proof(draft.hash)); assert.equal(g.hasCommit(draft.hash), true);
  const received = graph(t); received.ingest(g.exportChanges(new ExportCursor()).added[0].bytes);
  rejectsWithoutMutation(received, () => received.addSignature(alice.proof(draft.hash)), 'NotPendingLocalCommit');
});

test('fragments validate all members and canonical structure atomically, even on duplicate receipt', t => {
  const root = atDepth(alice), head = atDepth(alice, [root.hash], 1), original = fragment(head, [head, root]);
  const cases = [
    ['InvalidItem', i => { i.commits[1].payload[0] ^= 1; }],
    ['InvalidItem', i => { i.commits[1].author = bob.author; }],
    ['InvalidEncoding', i => { i.commits.push(copy(root)); }],
    ['InvalidEncoding', i => { i.boundary = [root.hash, root.hash]; }],
    ['InvalidItem', i => { i.head = root.hash; }],
    ['InvalidEncoding', i => { i.commits = [root]; }],
    ['MissingSignature', i => { i.signatures = []; }],
    ['InvalidSignature', i => { i.signatures[0].signature[0] ^= 1; }],
  ];
  for (const duplicate of [false, true]) {
    const g = graph(t); if (duplicate) g.ingest(encodeFixture(original));
    for (const [code, tamper] of cases) {
      const item = copy(original); tamper(item);
      rejectsWithoutMutation(g, () => g.ingest(encodeFixture(item)), code);
    }
    assertReport(g.ingest(encodeFixture(original)), [itemId(original)]);
    assert.equal(g.hasCommit(root.hash), true); assert.equal(g.hasCommit(head.hash), true);
  }
});

for (const trigger of ['receipt', 'local signing']) test(`deferred invalid fragment rejection retains valid dependency on ${trigger}`, t => {
  const g = graph(t), x = atDepth(bob), b = atDepth(bob, [], 1), middle = atDepth(alice, [b.hash, x.hash]), head = atDepth(alice, [middle.hash], 1);
  const malformed = fragment(head, [head, middle], [b.hash]), wire = encodeFixture(malformed);
  if (trigger === 'local signing') assert.equal(g.commit(bob.author, x.payload).hash, x.hash);
  g.ingest(encodeFixture(signedCommit(b, bob)));
  assertReport(g.ingest(wire), [], 1); assertReport(g.ingest(wire.slice()), [], 1);
  assert.deepEqual(g.missingDependencies(), [x.hash]);
  const report = trigger === 'receipt' ? g.ingest(encodeFixture(signedCommit(x, bob))) : g.addSignature(bob.proof(x.hash));
  assertIds(report.admitted, [itemId(signedCommit(x))]); assert.equal(report.pending, 0);
  assert.equal(report.rejected.length, 1); assert.deepEqual(report.rejected[0].item, itemId(malformed));
  assertErrorCode(report.rejected[0].error, 'InvalidFragment');
  assert.deepEqual(g.missingDependencies(), []); assert.equal(g.pendingItems(), 0);
  for (const c of [x, b]) assert.equal(g.hasCommit(c.hash), true);
  for (const c of [middle, head]) assert.equal(g.hasCommit(c.hash), false);
  const delivered = g.changes(new MaterializationCursor()); assert.equal(delivered.changes.length, 2);
  rejectsWithoutMutation(g, () => g.ingest(wire), 'InvalidFragment');
  const valid = fragment(head, [head, middle, x], [b.hash], [alice.proof(head.hash), bob.proof(x.hash)]);
  assertReport(g.ingest(encodeFixture(valid)), [itemId(valid)]);
  assert.deepEqual(events(g.changes(delivered.cursor)), [[middle.hash, true, 'neither'], [head.hash, true, 'neither']]);
  rejectsWithoutMutation(g, () => g.ingest(wire), 'InvalidFragment');
});

test('queued valid and malformed certificates sharing identity process independently', t => {
  const x = atDepth(bob), b = atDepth(bob, [], 1), middle = atDepth(alice, [b.hash, x.hash]), head = atDepth(alice, [middle.hash], 1);
  const other = atDepth(bob, [b.hash]), absent = atDepth(carol), waiting = atDepth(carol, [absent.hash]);
  const malformed = fragment(head, [head, middle], [b.hash]);
  const valid = fragment(head, [head, middle, x], [b.hash], [alice.proof(head.hash), bob.proof(x.hash)]);
  for (const validFirst of [false, true]) {
    const g = graph(t); g.ingest(encodeFixture(signedCommit(x, bob)));
    for (const [index, item] of (validFirst ? [valid, malformed] : [malformed, valid]).entries()) assertReport(g.ingest(encodeFixture(item)), [], index + 1);
    assertReport(g.ingest(encodeFixture(signedCommit(other, bob))), [], 3);
    assertReport(g.ingest(encodeFixture(signedCommit(waiting, carol))), [], 4);
    assert.deepEqual(g.missingDependencies(), [b.hash, absent.hash].sort());
    const report = g.ingest(encodeFixture(signedCommit(b, bob)));
    assertIds(report.admitted, [itemId(signedCommit(b)), itemId(signedCommit(other)), itemId(valid)]);
    assert.equal(report.rejected.length, 1); assert.deepEqual(report.rejected[0].item, itemId(malformed));
    assertErrorCode(report.rejected[0].error, 'InvalidFragment'); assert.equal(report.pending, 1);
    assert.deepEqual(g.missingDependencies(), [absent.hash]);
    for (const c of [x, b, middle, head, other]) assert.equal(g.hasCommit(c.hash), true);
    assert.equal(g.hasCommit(waiting.hash), false);
    assertReport(g.ingest(encodeFixture(signedCommit(absent, carol))), [itemId(signedCommit(absent)), itemId(signedCommit(waiting))]);
    assert.deepEqual(g.missingDependencies(), []);
  }
});

test('disposal is idempotent and every graph method fails with Disposed', t => {
  const g = graph(t), commit = atDepth(alice); g.dispose(); g.dispose();
  for (const operation of [
    () => g.commit(alice.author, bytes('x')), () => g.describeCommit(alice.author, bytes('x')),
    () => g.addSignature(alice.proof(commit.hash)), () => g.ingest(encodeFixture(signedCommit(commit))),
    () => g.viewPoint(), () => g.signedViewPoint(), () => g.setWritePermissions(new Map()),
    () => g.changes(new MaterializationCursor()), () => g.exportChanges(new ExportCursor()),
    () => g.setDecodeLimits({}),
    () => g.hasCommit(commit.hash), () => g.isAncestor(commit.hash, commit.hash),
    () => g.pendingItems(), () => g.missingDependencies(),
  ]) throwsCode(operation, 'Disposed');
});
