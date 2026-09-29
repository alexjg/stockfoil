import assert from 'node:assert/strict';
import test from 'node:test';
import {
  authorId, commitHash, describeCommit, ExportCursor, MaterializationCursor,
} from 'stockfoil';
import {
  alice, bob, carol, unknown, bytes, copy, atDepth, signedCommit, fragment,
  itemId, assertIds, assertReport, assertErrorCode, throwsCode, graph, events,
  assertTopological, rejectsWithoutMutation,
} from './support.mjs';

test('local drafts are delivered immediately; signing authenticates without redelivery', (t) => {
  const g = graph(t, [alice]);
  const payload = bytes('local edit');
  const expected = describeCommit(alice.author, [], payload);
  const draft = g.commit(alice.author, payload);
  assert.equal(draft.hash, expected.hash);
  assert.ok(draft.signingBytes instanceof Uint8Array);
  assert.equal(commitHash(draft.signingBytes), draft.hash);
  assert.equal(draft.signingBytes.length, 32);
  assertReport(draft.report);
  assert.equal(g.hasCommit(draft.hash), false);
  assert.equal(g.pendingItems(), 0);
  assert.deepEqual(g.missingDependencies(), []);
  assert.deepEqual(g.viewPoint().heads, [draft.hash]);
  assert.deepEqual(g.signedViewPoint().heads, []);
  assert.deepEqual(g.exportChanges(new ExportCursor()).added, []);

  const initial = new MaterializationCursor();
  const signed = g.changes(initial, g.signedViewPoint());
  assert.deepEqual(events(signed), [[draft.hash, true, 'neither']]);
  const working = g.changes(initial);
  assert.deepEqual(events(working), [[draft.hash, true, 'after']]);
  const evidence = { author: alice.author, commit: draft.hash, signature: alice.signBytes(draft.signingBytes) };
  payload.fill(255);
  draft.signingBytes.fill(0);
  assert.deepEqual(working.changes[0].commit, expected);
  assertReport(g.addSignature(evidence), [itemId(signedCommit(expected))]);
  evidence.signature.fill(0);
  assert.equal(g.hasCommit(draft.hash), true);
  assert.deepEqual(g.signedViewPoint(), g.viewPoint());
  assert.deepEqual(g.changes(working.cursor).changes, []);
  assert.deepEqual(events(g.changes(signed.cursor, g.signedViewPoint())), [[draft.hash, false, 'after']]);
  assert.deepEqual(g.exportChanges(new ExportCursor()).added[0].commit, expected);
});

test('delivery includes denied authors; permissions change participation, not authentication', (t) => {
  const g = graph(t, [alice]);
  const root = atDepth(alice);
  const denied = atDepth(bob, [root.hash]);
  const tip = atDepth(alice, [denied.hash]);
  for (const [commit, who] of [[root, alice], [denied, bob], [tip, alice]]) g.ingest(signedCommit(commit, who));
  const first = g.changes(new MaterializationCursor());
  assert.deepEqual(events(first), [[root.hash, true, 'after'], [denied.hash, true, 'neither'], [tip.hash, true, 'after']]);
  const published = g.exportChanges(new ExportCursor());
  assertIds(published.added.map(itemId), [root, denied, tip].map((commit) => itemId(signedCommit(commit))));
  assert.equal(g.hasCommit(denied.hash), true, 'denied writes are authenticated/exportable');

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

test('viewpoints, permission maps, payloads and exported certificates are owned copies', (t) => {
  const g = graph(t);
  const root = atDepth(alice);
  const incoming = signedCommit(root);
  g.ingest(incoming);
  const permission = { through: [root.hash] };
  const permissions = new Map([[alice.author, permission]]);
  g.setWritePermissions(permissions);
  const saved = g.viewPoint();
  const expectedView = copy(saved);
  const batch = g.changes(new MaterializationCursor());
  const exported = g.exportChanges(new ExportCursor());
  const expectedItem = copy(exported.added[0]);
  permissions.clear();
  permission.through.length = 0;
  incoming.commit.payload.fill(255);
  incoming.commit.parents.push(unknown);
  incoming.signatures[0].signature.fill(0);
  assert.deepEqual(g.viewPoint(), expectedView);
  assert.deepEqual(batch.changes[0].commit, root);
  assert.deepEqual(exported.added[0], expectedItem);

  saved.heads.length = 0;
  saved.writePermissions.get(alice.author).through.length = 0;
  batch.changes[0].commit.payload.fill(255);
  batch.changes[0].commit.parents.push(unknown);
  exported.added[0].commit.payload.fill(255);
  exported.added[0].signatures[0].signature.fill(0);
  assert.deepEqual(g.viewPoint(), expectedView);
  assert.deepEqual(g.changes(new MaterializationCursor()).changes[0].commit, root);
  assert.deepEqual(g.exportChanges(new ExportCursor()).added[0], expectedItem);

  const retained = g.changes(new MaterializationCursor());
  const retainedExport = g.exportChanges(new ExportCursor());
  g.commit(alice.author, new Uint8Array(1024 * 1024)); // May grow Wasm memory.
  g.setWritePermissions(new Map());
  g.dispose();
  assert.deepEqual(retained.changes[0].commit, root);
  assert.deepEqual(retainedExport.added[0], expectedItem);
  const receiver = graph(t);
  assertReport(receiver.ingest(retainedExport.added[0]), [itemId(expectedItem)]);
});

test('historical reads retain delivery and do not mutate the live cursor', (t) => {
  const g = graph(t, [alice]);
  const root = g.commit(alice.author, bytes('root'));
  const old = g.viewPoint();
  const child = g.commit(alice.author, bytes('child'));
  const current = g.viewPoint();
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

test('cloned cursors work on equivalent graphs with different ingestion order', (t) => {
  const left = graph(t, [alice, bob]);
  const right = graph(t);
  const a = atDepth(alice);
  const b = atDepth(bob);
  const join = atDepth(alice, [a.hash, b.hash]);
  left.ingest(signedCommit(a));
  left.ingest(signedCommit(b, bob));
  const cursor = left.changes(new MaterializationCursor()).cursor;
  left.ingest(signedCommit(join));
  right.ingest(signedCommit(b, bob));
  right.ingest(signedCommit(a));
  right.ingest(signedCommit(join));
  const target = left.viewPoint();
  const expected = left.changes(cursor, target);
  assert.deepEqual(events(expected), [[join.hash, true, 'after']]);
  assert.deepEqual(right.changes(cursor.clone(), target).changes, expected.changes);
  assert.deepEqual(right.changes(expected.cursor, target).changes, []);
  assert.deepEqual(right.viewPoint().writePermissions, new Map());
  left.dispose();
  assert.deepEqual(events(right.changes(cursor.clone(), target)), events(expected));
});

test('unknown viewpoint heads and missing cursor history fail without advancing cursors', (t) => {
  const g = graph(t, [alice]);
  const initial = new MaterializationCursor();
  const draft = g.commit(alice.author, bytes('known'));
  for (const writePermissions of [new Map(), g.viewPoint().writePermissions]) {
    rejectsWithoutMutation(g, () => g.changes(initial, { heads: [unknown], writePermissions }), 'MissingCommit');
  }
  const batch = g.changes(initial, { heads: [], writePermissions: new Map() });
  assert.deepEqual(events(batch), [[draft.hash, true, 'neither']]);
  const empty = graph(t);
  throwsCode(() => empty.changes(batch.cursor), 'MissingCommit');
  assert.deepEqual(g.changes(batch.cursor, { heads: [], writePermissions: new Map() }).changes, []);
  assert.deepEqual(events(g.changes(initial)), [[draft.hash, true, 'after']]);
});

test('batches are topological and replayable after application failure, with no traversal borrow', (t) => {
  const g = graph(t, [alice, bob]);
  const root = atDepth(alice);
  const left = atDepth(alice, [root.hash]);
  const right = atDepth(bob, [root.hash]);
  const join = atDepth(alice, [left.hash, right.hash, left.hash]);
  for (const [commit, who] of [[join, alice], [right, bob], [left, alice], [root, alice]]) g.ingest(signedCommit(commit, who));
  const cursor = new MaterializationCursor();
  const target = g.viewPoint();
  const batch = g.changes(cursor);
  assertTopological(batch);
  assert.equal(batch.changes.length, 4);
  assert.deepEqual(batch.changes.at(-1).commit.parents, join.parents, 'parent multiplicity is preserved');
  assert.equal(g.isAncestor(root.hash, join.hash), true);
  assert.equal(g.isAncestor(left.hash, right.hash), false);
  assert.throws(() => {
    for (const { commit } of batch.changes) {
      assert.equal(g.hasCommit(commit.hash), true);
      g.setWritePermissions(new Map()); // No Rust borrow survives changes().
      throw new Error('application failed after a partial update');
    }
  }, /application failed/);
  assert.deepEqual(g.changes(cursor.clone(), target).changes, batch.changes);
  assert.deepEqual(g.changes(batch.cursor, target).changes, []);
});

test('export cursors support full, noop, retry, clone, independent destinations and increments', (t) => {
  const g = graph(t);
  const empty = new ExportCursor();
  const first = g.exportChanges(empty);
  assert.deepEqual(first.added, []);
  assert.deepEqual(first.retired, []);
  const root = atDepth(alice);
  const child = atDepth(alice, [root.hash]);
  for (const commit of [root, child]) g.ingest(signedCommit(commit));
  const full = g.exportChanges(first.cursor);
  assertIds(full.added.map(itemId), [root, child].map((commit) => itemId(signedCommit(commit))));
  assert.deepEqual(full.retired, []);
  for (const cursor of [empty, first.cursor, first.cursor.clone(), new ExportCursor()]) {
    assert.deepEqual(g.exportChanges(cursor).added, full.added);
  }
  assert.deepEqual(g.exportChanges(full.cursor).added, []);
  const tip = atDepth(bob, [child.hash]);
  g.ingest(signedCommit(tip, bob));
  const increment = g.exportChanges(full.cursor.clone());
  assert.deepEqual(increment.added, [signedCommit(tip, bob)]);
  assert.deepEqual(increment.retired, []);
  assert.deepEqual(g.exportChanges(increment.cursor).added, []);
  const receiver = graph(t);
  for (const item of [...full.added, ...increment.added].reverse()) receiver.ingest(item);
  assert.equal(receiver.pendingItems(), 0);
  assert.deepEqual(receiver.signedViewPoint(), g.signedViewPoint());
});

test('a checkpoint fragment replaces loose certificates without changing materialization', (t) => {
  const g = graph(t, [alice]);
  const root = atDepth(alice);
  const head = atDepth(alice, [root.hash], 1);
  g.ingest(signedCommit(root));
  g.ingest(signedCommit(head)); // A checkpoint announcement is not a fragment certificate.
  const before = g.exportChanges(new ExportCursor());
  const materialized = g.changes(new MaterializationCursor());
  const view = g.viewPoint();
  const certificate = fragment(head, [head, root]);
  assertReport(g.ingest(certificate), [itemId(certificate)]);
  const change = g.exportChanges(before.cursor);
  assertIds(change.added.map(itemId), [itemId(certificate)]);
  assertIds(change.retired, before.added.map(itemId));
  assert.deepEqual(g.viewPoint(), view);
  assert.deepEqual(g.changes(materialized.cursor).changes, []);
  assert.deepEqual(g.exportChanges(change.cursor).added, []);
  assert.deepEqual(g.exportChanges(before.cursor.clone()).retired, change.retired);
  const receiver = graph(t);
  for (const item of [...before.added, ...change.added]) receiver.ingest(item);
  assert.equal(receiver.hasCommit(root.hash), true);
  assert.equal(receiver.hasCommit(head.hash), true);
});

test('deeper fragments retire earlier fragments; saved owned exports survive pruning', (t) => {
  const g = graph(t);
  const root = atDepth(alice, [], 1);
  const head = atDepth(alice, [root.hash], 2);
  const old = fragment(root, [root]);
  const next = fragment(head, [head, root]);
  g.ingest(old);
  const saved = g.exportChanges(new ExportCursor());
  g.ingest(next);
  const change = g.exportChanges(saved.cursor);
  assertIds(change.added.map(itemId), [itemId(next)]);
  assertIds(change.retired, [itemId(old)]);
  assert.deepEqual(g.exportChanges(change.cursor).added, []);
  g.dispose();
  const receiver = graph(t);
  assertReport(receiver.ingest(saved.added[0]), [itemId(old)]);
  assertReport(receiver.ingest(change.added[0]), [itemId(next)]);
  assert.equal(receiver.hasCommit(root.hash), true);
  assert.equal(receiver.hasCommit(head.hash), true);
});

test('different sufficient signature evidence and permissions do not republish an item identity', (t) => {
  const g = graph(t);
  const root = atDepth(alice);
  const head = atDepth(bob, [root.hash], 1);
  const first = fragment(head, [root, head], [], [alice.proof(root.hash), bob.proof(head.hash)]);
  g.ingest(first);
  const exported = g.exportChanges(new ExportCursor());
  const second = fragment(head, [head, root], [], [alice.proof(head.hash), bob.proof(head.hash)]);
  g.ingest(second);
  const fresh = g.exportChanges(new ExportCursor());
  assert.notDeepEqual(fresh.added[0].signatures, exported.added[0].signatures);
  g.setWritePermissions(new Map([[alice.author, 'unbounded'], [bob.author, 'unbounded']]));
  const noop = g.exportChanges(exported.cursor);
  assert.deepEqual(noop.added, []);
  assert.deepEqual(noop.retired, []);
  for (const item of [exported.added[0], fresh.added[0]]) assert.equal(graph(t).ingest(item).pending, 0);
});

test('reverse fan-in receipts queue, deduplicate and drain; payload inputs are copied', (t) => {
  const g = graph(t, [alice, bob]);
  const root = atDepth(alice);
  const left = atDepth(alice, [root.hash]);
  const right = atDepth(bob, [root.hash]);
  const join = atDepth(alice, [left.hash, right.hash, left.hash]);
  const tip = atDepth(alice, [join.hash]);
  const items = [signedCommit(tip), signedCommit(join), signedCommit(right, bob), signedCommit(left)];
  for (const [index, item] of items.entries()) {
    assertReport(g.ingest(item), [], index + 1);
    assertReport(g.ingest(copy(item)), [], index + 1);
    assert.equal(g.hasCommit(item.commit.hash), false);
    item.commit.payload.fill(255);
    item.commit.parents.length = 0;
    item.signatures[0].signature.fill(0);
  }
  assert.deepEqual(g.missingDependencies(), [root.hash, left.hash, right.hash, join.hash].sort());
  const missing = g.missingDependencies();
  missing.length = 0;
  assert.equal(g.missingDependencies().length, 4);
  assert.deepEqual(g.viewPoint().heads, []);
  assert.deepEqual(g.changes(new MaterializationCursor()).changes, []);
  assert.deepEqual(g.exportChanges(new ExportCursor()).added, []);
  const commits = [root, left, right, join, tip];
  assertReport(g.ingest(signedCommit(root)), commits.map((commit) => itemId(signedCommit(commit))));
  assert.deepEqual(g.missingDependencies(), []);
  assert.equal(g.pendingItems(), 0);
  const batch = g.changes(new MaterializationCursor());
  assertTopological(batch);
  assert.equal(batch.changes.length, commits.length);
  for (const commit of commits) {
    assert.equal(g.hasCommit(commit.hash), true);
    assert.deepEqual(batch.changes.find((change) => change.commit.hash === commit.hash).commit, commit);
  }
  assertReport(g.ingest(signedCommit(tip)), [itemId(signedCommit(tip))]);
  assert.deepEqual(g.changes(batch.cursor).changes, []);
});

test('signing a local checkpoint wakes received dependants of promoted interior drafts', (t) => {
  const g = graph(t, [alice, bob]);
  const root = atDepth(alice);
  const middle = atDepth(alice, [root.hash]);
  const checkpoint = atDepth(alice, [middle.hash], 1);
  const child = atDepth(bob, [middle.hash]);
  const tip = atDepth(bob, [child.hash]);
  assertReport(g.ingest(signedCommit(tip, bob)), [], 1);
  const incoming = signedCommit(child, bob);
  assertReport(g.ingest(incoming), [], 2);
  incoming.commit.payload.fill(255);
  for (const commit of [root, middle, checkpoint]) {
    const draft = g.commit(alice.author, commit.payload);
    assert.equal(draft.hash, commit.hash);
    assertReport(draft.report, [], 2);
    assert.equal(g.hasCommit(commit.hash), false);
  }
  assert.deepEqual(g.missingDependencies(), [middle.hash, child.hash].sort());
  assert.deepEqual(g.exportChanges(new ExportCursor()).added, []);
  const working = g.changes(new MaterializationCursor());
  const report = g.addSignature(alice.proof(checkpoint.hash));
  assertReport(report, [itemId(fragment(checkpoint, [root, middle, checkpoint])), itemId(signedCommit(child)), itemId(signedCommit(tip))]);
  assert.deepEqual(g.missingDependencies(), []);
  const changes = g.changes(working.cursor);
  assert.deepEqual(events(changes), [[child.hash, true, 'after'], [tip.hash, true, 'after']]);
  assert.deepEqual(changes.changes[0].commit.payload, child.payload);
  for (const commit of [root, middle, checkpoint, child, tip]) assert.equal(g.hasCommit(commit.hash), true);
  const receiver = graph(t);
  for (const item of g.exportChanges(new ExportCursor()).added.reverse()) receiver.ingest(item);
  assert.equal(receiver.pendingItems(), 0);
  assert.equal(receiver.hasCommit(tip.hash), true);
});

test('describeCommit owns inputs and outputs, but recomputing hashes does not authenticate edits', (t) => {
  const g = graph(t);
  const parents = [atDepth(bob).hash, atDepth(carol).hash];
  const payload = bytes('original');
  const original = describeCommit(alice.author, parents, payload);
  const saved = copy(original);
  parents.reverse();
  payload.fill(255);
  assert.deepEqual(original, saved);
  original.payload.fill(0);
  original.parents.length = 0;
  assert.deepEqual(describeCommit(alice.author, saved.parents, bytes('original')), saved);
  for (const modified of [
    describeCommit(alice.author, saved.parents, bytes('tampered')),
    describeCommit(alice.author, [...saved.parents].reverse(), saved.payload),
    describeCommit(bob.author, saved.parents, saved.payload),
  ]) {
    assert.notEqual(modified.hash, saved.hash);
    const evidence = alice.proof(saved.hash);
    evidence.commit = modified.hash; // Correct derived hashes, but an old signature.
    rejectsWithoutMutation(g, () => g.ingest({ kind: 'commit', commit: modified, signatures: [evidence] }), 'InvalidSignature');
  }
});

test('skipped export covers coalesce instead of reporting transient loose inventory', (t) => {
  const g = graph(t);
  const root = atDepth(alice);
  const middle = atDepth(alice, [root.hash]);
  const head = atDepth(alice, [middle.hash], 1);
  g.ingest(signedCommit(root));
  const saved = g.exportChanges(new ExportCursor());
  g.ingest(signedCommit(middle)); // No publication of this intermediate cover.
  assert.equal(g.commit(alice.author, head.payload).hash, head.hash);
  g.addSignature(alice.proof(head.hash));
  const delta = g.exportChanges(saved.cursor);
  assertIds(delta.added.map(itemId), [itemId(fragment(head, [root, middle, head]))]);
  assertIds(delta.retired, [itemId(signedCommit(root))]);
  assert.deepEqual(g.exportChanges(saved.cursor.clone()).added, delta.added);
  assert.deepEqual(g.exportChanges(delta.cursor).retired, []);
  const receiver = graph(t);
  for (const item of [...saved.added, ...delta.added]) receiver.ingest(item);
  assert.equal(receiver.hasCommit(middle.hash), true);
  assert.equal(receiver.hasCommit(head.hash), true);
});

test('identifier and decoded input validation reports stable StockfoilError instances', (t) => {
  const g = graph(t);
  assert.equal(authorId(alice.publicKey), alice.author);
  assert.equal(commitHash(new Uint8Array(32).fill(171)), 'ab'.repeat(32));
  for (const convert of [authorId, commitHash]) {
    for (const value of [new Uint8Array(31), new Uint8Array(33), [], '00']) throwsCode(() => convert(value), 'InvalidInput');
  }
  for (const hash of ['AB'.repeat(32), 'g'.repeat(64), '0'.repeat(63), '0'.repeat(65), 123, null]) {
    rejectsWithoutMutation(g, () => g.hasCommit(hash), 'InvalidInput');
    throwsCode(() => g.isAncestor(unknown, hash), 'InvalidInput');
  }
  for (const operation of [
    () => g.commit(alice.author, [1, 2]),
    () => describeCommit(alice.author.toUpperCase(), [], bytes('x')),
    () => describeCommit(alice.author, [unknown.toUpperCase()], bytes('x')),
    () => g.setWritePermissions({ [alice.author]: 'unbounded' }),
    () => g.setWritePermissions(new Map([[alice.author, 'write']])),
    () => g.setWritePermissions(new Map([[alice.author, { through: ['bad'] }]])),
    () => g.changes(new MaterializationCursor(), { heads: [], writePermissions: {} }),
    () => g.changes(new ExportCursor()),
    () => g.exportChanges(new MaterializationCursor()),
    () => g.changes({}),
    () => g.ingest({ kind: 'unknown' }),
    () => g.ingest({ kind: 'commit' }),
  ]) rejectsWithoutMutation(g, operation, 'InvalidInput');
});

test('payload, metadata and every signature are validated before mutation, including duplicates and queued items', (t) => {
  const root = atDepth(alice);
  const other = atDepth(bob);
  const child = atDepth(alice, [root.hash, other.hash, root.hash]);
  const original = signedCommit(child);
  const cases = [
    ['PayloadHashMismatch', (item) => { item.commit.payload[0] ^= 1; }],
    ['PayloadHashMismatch', (item) => { item.commit.payloadHash = unknown; }],
    ['CommitHashMismatch', (item) => { item.commit.hash = unknown; }],
    ['CommitHashMismatch', (item) => { item.commit.author = bob.author; }],
    ['CommitHashMismatch', (item) => { item.commit.parents = [other.hash, root.hash, root.hash]; }],
    ['CommitHashMismatch', (item) => { item.commit.parents = [root.hash, other.hash]; }],
    ['InvalidInput', (item) => { item.commit.payloadHash = item.commit.payloadHash.toUpperCase(); }],
    ['MissingSignature', (item) => { item.signatures = []; }],
    ['MissingSignature', (item) => { item.signatures = [bob.proof(child.hash)]; }],
    ['InvalidSignature', (item) => { item.signatures[0].signature[0] ^= 1; }],
    ['InvalidSignature', (item) => { item.signatures[0].author = bob.author; }],
    ['InvalidInput', (item) => { item.signatures[0].signature = new Uint8Array(63); }],
    ['InvalidItem', (item) => { item.signatures = [alice.proof(root.hash)]; }],
    ['InvalidSignature', (item) => {
      const extra = bob.proof(child.hash);
      extra.signature[0] ^= 1;
      item.signatures.push(extra); // A sufficient good proof must not hide a bad extra one.
    }],
    ['InvalidPublicKey', (item) => {
      item.signatures.push({ author: '02'.repeat(32), commit: child.hash, signature: new Uint8Array(64) });
    }],
  ];
  for (const state of ['fresh', 'queued', 'admitted']) {
    const g = graph(t);
    if (state === 'admitted') {
      g.ingest(signedCommit(root));
      g.ingest(signedCommit(other, bob));
    }
    if (state !== 'fresh') g.ingest(original);
    for (const [code, tamper] of cases) {
      const item = copy(original);
      tamper(item);
      rejectsWithoutMutation(g, () => g.ingest(item), code);
    }
    // Valid data remains usable after all failed attempts.
    if (state !== 'admitted') {
      g.ingest(signedCommit(root));
      g.ingest(signedCommit(other, bob));
    }
    assertReport(g.ingest(original), [itemId(original)]);
    assert.equal(g.hasCommit(child.hash), true);
  }
});

test('local signature failures leave drafts unsigned and nonexportable', (t) => {
  const g = graph(t, [alice]);
  const draft = g.commit(alice.author, bytes('draft'));
  const bad = alice.proof(draft.hash);
  bad.signature[0] ^= 1;
  rejectsWithoutMutation(g, () => g.addSignature(bad), 'InvalidSignature');
  rejectsWithoutMutation(g, () => g.addSignature({ ...bad, signature: new Uint8Array(65) }), 'InvalidInput');
  rejectsWithoutMutation(g, () => g.addSignature({ ...bad, author: '02'.repeat(32) }), 'InvalidPublicKey');
  rejectsWithoutMutation(g, () => g.addSignature(alice.proof(unknown)), 'NotPendingLocalCommit');
  assert.equal(g.hasCommit(draft.hash), false);
  assert.deepEqual(g.exportChanges(new ExportCursor()).added, []);
  g.addSignature(alice.proof(draft.hash));
  assert.equal(g.hasCommit(draft.hash), true);
  const received = graph(t);
  received.ingest(g.exportChanges(new ExportCursor()).added[0]);
  rejectsWithoutMutation(received, () => received.addSignature(alice.proof(draft.hash)), 'NotPendingLocalCommit');
});

test('fragments validate all members and canonical structure atomically, even on duplicate receipt', (t) => {
  const root = atDepth(alice);
  const head = atDepth(alice, [root.hash], 1);
  const original = fragment(head, [head, root]);
  const cases = [
    ['PayloadHashMismatch', (item) => { item.commits[1].payload[0] ^= 1; }],
    ['CommitHashMismatch', (item) => { item.commits[1].author = bob.author; }],
    ['InvalidFragment', (item) => { item.commits.push(copy(root)); }],
    ['InvalidFragment', (item) => { item.boundary = [root.hash, root.hash]; }],
    ['InvalidFragment', (item) => { item.head = root.hash; }],
    ['InvalidFragment', (item) => { item.commits = [root]; }],
    ['MissingSignature', (item) => { item.signatures = []; }],
    ['InvalidSignature', (item) => { item.signatures[0].signature[0] ^= 1; }],
  ];
  for (const duplicate of [false, true]) {
    const g = graph(t);
    if (duplicate) g.ingest(original);
    for (const [code, tamper] of cases) {
      const item = copy(original);
      tamper(item);
      rejectsWithoutMutation(g, () => g.ingest(item), code);
    }
    assertReport(g.ingest(original), [itemId(original)]);
    assert.equal(g.hasCommit(root.hash), true);
    assert.equal(g.hasCommit(head.hash), true);
  }
});

for (const trigger of ['receipt', 'local signing']) {
  test(`deferred invalid fragment rejection retains the valid dependency on ${trigger}`, (t) => {
    // x is not in b's history. Omitting x is locally verifiable, but cannot be
    // admitted once the external dependency reveals the true fragment range.
    const g = graph(t);
    const x = atDepth(bob);
    const b = atDepth(bob, [], 1);
    const middle = atDepth(alice, [b.hash, x.hash]);
    const head = atDepth(alice, [middle.hash], 1);
    const malformed = fragment(head, [head, middle], [b.hash]);
    if (trigger === 'local signing') assert.equal(g.commit(bob.author, x.payload).hash, x.hash);
    g.ingest(signedCommit(b, bob));
    assertReport(g.ingest(malformed), [], 1);
    assertReport(g.ingest(copy(malformed)), [], 1);
    assert.deepEqual(g.missingDependencies(), [x.hash]);
    const report = trigger === 'receipt' ? g.ingest(signedCommit(x, bob)) : g.addSignature(bob.proof(x.hash));
    assertIds(report.admitted, [itemId(signedCommit(x))]);
    assert.equal(report.pending, 0);
    assert.equal(report.rejected.length, 1);
    assert.deepEqual(report.rejected[0].item, itemId(malformed));
    assertErrorCode(report.rejected[0].error, 'InvalidFragment');
    assert.equal(g.pendingItems(), 0);
    assert.deepEqual(g.missingDependencies(), []);
    assert.equal(g.hasCommit(x.hash), true);
    assert.equal(g.hasCommit(b.hash), true);
    for (const commit of [middle, head]) assert.equal(g.hasCommit(commit.hash), false);
    const delivered = g.changes(new MaterializationCursor());
    assert.equal(delivered.changes.length, 2);
    rejectsWithoutMutation(g, () => g.ingest(malformed), 'InvalidFragment');

    const valid = fragment(head, [head, middle, x], [b.hash], [alice.proof(head.hash), bob.proof(x.hash)]);
    assertReport(g.ingest(valid), [itemId(valid)]);
    assert.equal(g.hasCommit(head.hash), true);
    assert.deepEqual(events(g.changes(delivered.cursor)), [[middle.hash, true, 'neither'], [head.hash, true, 'neither']]);
    // An apparent duplicate must still undergo canonical range validation.
    rejectsWithoutMutation(g, () => g.ingest(malformed), 'InvalidFragment');
  });
}

test('queued valid and malformed certificates with one identity are processed independently', (t) => {
  const x = atDepth(bob);
  const b = atDepth(bob, [], 1);
  const middle = atDepth(alice, [b.hash, x.hash]);
  const head = atDepth(alice, [middle.hash], 1);
  const other = atDepth(bob, [b.hash]);
  const absent = atDepth(carol);
  const waiting = atDepth(carol, [absent.hash]);
  const malformed = fragment(head, [head, middle], [b.hash]);
  const valid = fragment(head, [head, middle, x], [b.hash], [alice.proof(head.hash), bob.proof(x.hash)]);
  for (const validFirst of [false, true]) {
    const g = graph(t);
    g.ingest(signedCommit(x, bob));
    const items = validFirst ? [valid, malformed] : [malformed, valid];
    for (const [index, item] of items.entries()) assertReport(g.ingest(item), [], index + 1);
    assertReport(g.ingest(signedCommit(other, bob)), [], 3);
    assertReport(g.ingest(signedCommit(waiting, carol)), [], 4);
    assert.deepEqual(g.missingDependencies(), [b.hash, absent.hash].sort());
    const report = g.ingest(signedCommit(b, bob));
    assertIds(report.admitted, [itemId(signedCommit(b)), itemId(signedCommit(other)), itemId(valid)]);
    assert.equal(report.rejected.length, 1);
    assert.deepEqual(report.rejected[0].item, itemId(malformed));
    assertErrorCode(report.rejected[0].error, 'InvalidFragment');
    assert.equal(report.pending, 1);
    assert.deepEqual(g.missingDependencies(), [absent.hash]);
    for (const commit of [x, b, middle, head, other]) assert.equal(g.hasCommit(commit.hash), true);
    assert.equal(g.hasCommit(waiting.hash), false);
    assertReport(g.ingest(signedCommit(absent, carol)), [itemId(signedCommit(absent)), itemId(signedCommit(waiting))]);
    assert.deepEqual(g.missingDependencies(), []);
  }
});

test('disposal is idempotent and all graph methods fail with Disposed', (t) => {
  const g = graph(t);
  g.dispose();
  g.dispose();
  const commit = atDepth(alice);
  for (const operation of [
    () => g.commit(alice.author, bytes('x')),
    () => g.addSignature(alice.proof(commit.hash)),
    () => g.ingest(signedCommit(commit)),
    () => g.viewPoint(), () => g.signedViewPoint(),
    () => g.setWritePermissions(new Map()),
    () => g.changes(new MaterializationCursor()),
    () => g.exportChanges(new ExportCursor()),
    () => g.hasCommit(commit.hash), () => g.isAncestor(commit.hash, commit.hash),
    () => g.pendingItems(), () => g.missingDependencies(),
  ]) throwsCode(operation, 'Disposed');
});
