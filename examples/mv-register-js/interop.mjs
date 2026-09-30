// Reproducible real-network integration test and runnable demo.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { setTimeout as sleep } from 'node:timers/promises';
import { ExportCursor } from 'stockfoil';
import { setSubductionLogLevel } from '@automerge/subduction';
import { MvRegisterMap } from './register.mjs';
import { Replica, bounded } from './transport.mjs';
import { forgeries } from './test/support.mjs';

// Normal disconnects and the wrong-peer test need not print transport warnings.
setSubductionLogLevel('error');
const root = fileURLToPath(new URL('../../', import.meta.url));
const build = spawnSync('cargo', ['build', '-p', 'stockfoil-mv-register', '--message-format=json'], {
  cwd: root, encoding: 'utf8', timeout: 300_000, maxBuffer: 16 * 1024 * 1024,
});
if (build.status !== 0) throw new Error(build.stderr || String(build.error));
const executable = build.stdout.split('\n').filter(Boolean).map(line => JSON.parse(line))
  .find(m => m.reason === 'compiler-artifact' && m.target.name === 'stockfoil-mv-register' && m.executable)?.executable;
assert.ok(executable, 'cargo did not report native helper executable');
const native = spawn(executable, ['--interop'], { cwd: root, stdio: ['pipe', 'pipe', 'inherit'] });
const exited = new Promise(resolve => {
  native.once('exit', (code, signal) => resolve({ code, signal }));
  native.once('error', error => resolve({ error }));
});
// Last-resort deadline for async protocol waits and cleanup. A synchronous Wasm
// stall would require an external process timeout; the test loops are bounded.
const watchdog = setTimeout(() => { native.kill('SIGKILL'); console.error('interop deadline exceeded'); process.exit(1); }, 150_000);
const lines = createInterface({ input: native.stdout });
const iterator = lines[Symbol.asyncIterator]();
async function response() {
  const line = await bounded(iterator.next(), 30_000);
  if (line.done) throw new Error('native helper exited before replying');
  return JSON.parse(line.value);
}
async function command(op, fields = {}) {
  native.stdin.write(JSON.stringify({ op, ...fields }) + '\n');
  return response();
}
const docs = [], replicas = [];
try {
  const config = await response();
  await assert.rejects(Replica.connect({ ...config, peerId: '00'.repeat(32) }));
  const doc = new MvRegisterMap(); docs.push(doc);
  const permissions = new Map([[config.author, 'unbounded'], [doc.author, 'unbounded']]);
  doc.setWritePermissions(permissions);
  await command('configure', { authors: [...permissions.keys()] });
  const replica = await Replica.connect(config); replicas.push(replica);
  async function converge(key, expected) {
    const end = Date.now() + 25_000;
    do {
      await replica.publish(doc);
      await command('exchange');
      await replica.sync();
      const pending = await replica.load(doc);
      const rust = (await command('get', { key })).values;
      if (!pending && JSON.stringify(doc.get(key)) === JSON.stringify(expected) && JSON.stringify(rust) === JSON.stringify(expected)) return;
      await sleep(20);
    } while (Date.now() < end);
    assert.fail(`replicas did not converge for ${key}`);
  }

  await command('set', { key: 'title', value: 'before' });
  await converge('title', ['before']);
  doc.set('shared', 'from-js');
  await converge('shared', ['from-js']);
  const historical = doc.viewPoint();
  // Both authors edit the same shared history, without loading the other edit.
  await command('set', { key: 'title', value: 'Hello' });
  doc.set('title', 'Howdy');
  await converge('title', ['Hello', 'Howdy']);
  assert.deepEqual(doc.getAt(historical, 'title'), ['before']);
  assert.deepEqual(doc.get('title'), ['Hello', 'Howdy']);

  // Exercise a certified fragment originating in JS, not just loose commits.
  let jsLast;
  for (let n = 0; n < 8192; n++) {
    const hash = doc.set('js-chain', String(n));
    const cover = doc.graph.exportChanges(new ExportCursor());
    if (cover.added.some(item => item.metadata.kind === 'fragment' && item.metadata.head === hash)) { jsLast = String(n); break; }
  }
  assert.notEqual(jsLast, undefined, 'no JS fragment within bounded chain');
  const published = await replica.publish(doc);
  assert.ok(published.some(item => item.metadata.kind === 'fragment'));
  await converge('js-chain', [jsLast]);
  const rustFragment = await command('makeFragment');
  await converge('rust-chain', [rustFragment.last]);
  // Inspect the authenticated graph's exported metadata, never parse its wire.
  assert.ok(doc.graph.exportChanges(new ExportCursor()).added.some(item =>
    item.metadata.kind === 'fragment' && item.metadata.head === rustFragment.head));

  // Fresh JS graph AND Subduction store download from the server, then ingest
  // in reverse blob order. The pending queue handles arbitrary arrival order.
  const fresh = new MvRegisterMap(); docs.push(fresh); fresh.setWritePermissions(permissions);
  const freshReplica = await Replica.connect(config); replicas.push(freshReplica);
  await freshReplica.sync();
  for (let n = 0; n < 100; n++) {
    await freshReplica.load(fresh, true);
    if (!fresh.graph.pendingItems() && fresh.get('rust-chain')[0] === rustFragment.last) break;
    await sleep(20);
  }
  assert.deepEqual(fresh.get('title'), ['Hello', 'Howdy']);
  assert.deepEqual(fresh.get('js-chain'), [jsLast]);
  assert.deepEqual(fresh.get('rust-chain'), [rustFragment.last]);
  assert.equal(fresh.graph.pendingItems(), 0);

  // Wire payload/signature forgery must be rejected by Stockfoil, even though
  // Subduction would happily store blobs signed by its own transport identity.
  const forger = new MvRegisterMap(); docs.push(forger);
  const hash = forger.set('title', 'authentic');
  for (const forged of forgeries(forger, hash, { key: 'title', value: 'authentic' })) {
    const victim = new MvRegisterMap(); docs.push(victim);
    victim.setWritePermissions(new Map([[forger.author, 'unbounded']]));
    assert.throws(() => victim.ingest([forged]));
    assert.deepEqual(victim.get('title'), []);
    assert.equal(victim.graph.hasCommit(hash), false);
  }

  // A fresh Rust graph/store must also recover JS-authored fragment contents.
  await command('fresh');
  await converge('title', ['Hello', 'Howdy']);
  await converge('js-chain', [jsLast]);
  await converge('rust-chain', [rustFragment.last]);
  console.log('Rust:       Hello, Howdy\nJavaScript: Hello, Howdy');
  console.log('Passed: bidirectional WebSocket sync, fragments, fresh replicas, historical reads, forgery rejection.');
} finally {
  for (const replica of replicas.reverse()) {
    try { await replica.close(); } catch (error) { console.error('disconnect:', error); }
  }
  for (const doc of docs) doc.dispose();
  try {
    if (native.exitCode === null && native.signalCode === null) await command('quit');
    const result = await bounded(exited, 5_000);
    if (result.code !== 0) throw new Error(`native helper failed: ${JSON.stringify(result)}`);
  } finally {
    native.kill('SIGKILL');
    lines.close();
    clearTimeout(watchdog);
  }
}
