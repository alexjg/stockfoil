import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const directory = fileURLToPath(new URL('../', import.meta.url));

test('browser WebCrypto signing, ingestion and materialization across generated entrypoints', { timeout: 60_000 }, async t => {
  const server = createServer(async (request, response) => {
    try {
      const path = new URL(request.url, 'http://localhost').pathname;
      if (path === '/') {
        response.setHeader('Content-Type', 'text/html');
        return response.end('<!doctype html><title>Stockfoil browser test</title>');
      }
      const filename = resolve(directory, '.' + decodeURIComponent(path));
      if (!filename.startsWith(resolve(directory, 'dist') + sep)) throw new Error('not found');
      const data = await readFile(filename);
      response.setHeader('Content-Type', path.endsWith('.wasm') ? 'application/wasm' : 'text/javascript');
      response.end(data);
    } catch { response.writeHead(404).end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => server.close(resolve)));
  const browser = await chromium.launch({ headless: true });
  t.after(() => browser.close());
  for (const entry of ['web', 'bundler', 'slimSync', 'slimAsync']) {
    const page = await browser.newPage();
    try {
      await page.goto(`http://127.0.0.1:${server.address().port}/`);
      const result = await page.evaluate(async entry => {
        const api = await import(`/dist/wrapper/esm/${entry.startsWith('slim') ? 'slim' : entry}.js`);
        if (entry === 'slimSync') api.initSync(await (await fetch('/dist/stockfoil.wasm')).arrayBuffer());
        if (entry === 'slimAsync') await api.default(new URL('/dist/stockfoil.wasm', location.href));
        const { CommitGraph, MaterializationCursor, ExportCursor, authorId, StockfoilError } = api;
        const keys = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify']);
        const author = authorId(new Uint8Array(await crypto.subtle.exportKey('raw', keys.publicKey)));
        const source = new CommitGraph(), receiver = new CommitGraph();
        try {
          const draft = source.commit(author, new TextEncoder().encode('browser'));
          const signature = new Uint8Array(await crypto.subtle.sign('Ed25519', keys.privateKey, draft.signingBytes));
          source.addSignature({ author, commit: draft.hash, signature });
          for (const item of source.exportChanges(new ExportCursor()).added) receiver.ingest(item);
          const denied = receiver.changes(new MaterializationCursor());
          receiver.setWritePermissions(new Map([[author, 'unbounded']]));
          const visible = receiver.changes(denied.cursor);
          let code;
          try { receiver.hasCommit('invalid'); } catch (error) {
            if (!(error instanceof StockfoilError)) throw error;
            code = error.code;
          }
          return {
            admitted: receiver.hasCommit(draft.hash),
            denied: denied.changes[0].visibility,
            visible: visible.changes[0].visibility,
            newlyDelivered: visible.changes[0].newlyDelivered,
            payload: new TextDecoder().decode(visible.changes[0].commit.payload),
            code,
          };
        } finally { source.dispose(); receiver.dispose(); }
      }, entry);
      assert.deepEqual(result, {
        admitted: true, denied: 'neither', visible: 'after', newlyDelivered: false,
        payload: 'browser', code: 'InvalidInput',
      }, entry);
    } finally { await page.close(); }
  }
});
