import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync, copyFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as root from 'stockfoil';
import * as slim from 'stockfoil/slim';

const directory = fileURLToPath(new URL('../', import.meta.url));
const run = (script, format = 'module', cwd = directory) => execFileSync(
  process.execPath, [`--input-type=${format}`, '-e', script],
  { cwd, encoding: 'utf8', timeout: 30_000, stdio: ['ignore', 'pipe', 'pipe'] },
);

test('root and slim graphs accept each other’s cursors, including returned cursors and clones', () => {
  const author = root.authorId(new Uint8Array(32));
  const a = new root.CommitGraph(), b = new slim.CommitGraph();
  try {
    a.commit(author, new Uint8Array([1]));
    b.commit(author, new Uint8Array([1]));
    const batch = a.changes(new slim.MaterializationCursor());
    assert.equal(batch.changes.length, 1);
    assert.deepEqual(b.changes(batch.cursor.clone()).changes, []);
    assert.deepEqual(a.changes(b.changes(new root.MaterializationCursor()).cursor).changes, []);
    assert.deepEqual(a.exportChanges(b.exportChanges(new root.ExportCursor()).cursor.clone()).added, []);
    assert.equal(root.StockfoilError, slim.StockfoilError);
    assert.throws(() => b.ingest({}), root.StockfoilError);
    assert.throws(() => a.hasCommit('invalid'), slim.StockfoilError);
  } finally { a.dispose(); b.dispose(); }
});

test('CommonJS root auto-initializes and shares its Wasm instance with CommonJS slim', () => {
  run(`
    const assert = require('node:assert/strict');
    const root = require('stockfoil'), slim = require('stockfoil/slim');
    const graph = new root.CommitGraph();
    assert.deepEqual(graph.changes(new slim.MaterializationCursor()).changes, []);
    assert.equal(root.StockfoilError, slim.StockfoilError);
    graph.dispose();
  `, 'commonjs');
});

test('slim supports synchronous and asynchronous initialization without importing root', () => {
  for (const initializer of ['initSync(bytes)', 'await init(bytes)']) {
    run(`
      import assert from 'node:assert/strict';
      import { readFileSync } from 'node:fs';
      import init, { initSync, CommitGraph, MaterializationCursor } from 'stockfoil/slim';
      const bytes = readFileSync(new URL(import.meta.resolve('stockfoil/wasm')));
      ${initializer};
      const graph = new CommitGraph();
      assert.deepEqual(graph.changes(new MaterializationCursor()).changes, []);
      graph.dispose();
    `);
  }
});

test('published tarball runs independently and its public declarations compile without private aliases', () => {
  const temporary = mkdtempSync(join(tmpdir(), 'stockfoil-package-'));
  try {
    const packed = JSON.parse(execFileSync('npm', ['pack', '--json', '--pack-destination', temporary], {
      cwd: directory, encoding: 'utf8', timeout: 30_000,
    }))[0];
    const names = new Set(packed.files.map(f => f.path));
    assert.ok(names.has('dist/stockfoil.wasm'));
    assert.ok(names.has('dist/wrapper/index.d.ts'));
    assert.ok(![...names].some(name => name.startsWith('src/') || name.startsWith('.wasm-bodge/')));
    writeFileSync(join(temporary, 'package.json'), JSON.stringify({ private: true, type: 'module' }));
    execFileSync('npm', ['install', '--ignore-scripts', '--no-audit', '--no-fund', join(temporary, packed.filename)], {
      cwd: temporary, encoding: 'utf8', timeout: 30_000,
    });
    copyFileSync(join(directory, 'test/consumer.ts'), join(temporary, 'consumer.ts'));
    execFileSync(join(directory, 'node_modules/.bin/tsc'), [
      '--noEmit', '--strict', '--target', 'ES2022', '--module', 'NodeNext', '--lib', 'ES2022,DOM', 'consumer.ts',
    ], { cwd: temporary, encoding: 'utf8', timeout: 30_000 });
    run(`
      import assert from 'node:assert/strict';
      import { CommitGraph, MaterializationCursor } from 'stockfoil';
      const graph = new CommitGraph();
      assert.deepEqual(graph.changes(new MaterializationCursor()).changes, []);
      graph.dispose();
      assert.throws(() => import.meta.resolve('stockfoil/bindings'), { code: 'ERR_PACKAGE_PATH_NOT_EXPORTED' });
    `, 'module', temporary);
    run(`const { CommitGraph } = require('stockfoil'); new CommitGraph().dispose();`, 'commonjs', temporary);
    const manifest = JSON.parse(readFileSync(join(temporary, 'node_modules/stockfoil/package.json'), 'utf8'));
    assert.equal(manifest.exports['./bindings'], undefined);
  } finally { rmSync(temporary, { recursive: true, force: true }); }
});
