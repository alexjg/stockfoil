import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const directory = fileURLToPath(new URL('../', import.meta.url));

test('browser WebCrypto signing, ingestion and materialization across generated entrypoints', { timeout: 120_000 }, async t => {
  const server = createServer(async (request, response) => {
    try {
      const path = new URL(request.url, 'http://localhost').pathname;
      if (path === '/') {
        response.setHeader('Content-Type', 'text/html');
        return response.end('<!doctype html><title>Stockfoil browser test</title>');
      }
      const filename = resolve(directory, '.' + decodeURIComponent(path));
      if (!filename.startsWith(resolve(directory, 'dist') + sep) && filename !== resolve(directory, 'test/runs-codec.mjs')) throw new Error('not found');
      const data = await readFile(filename);
      response.setHeader('Content-Type', path.endsWith('.wasm') ? 'application/wasm' : 'text/javascript');
      response.end(data);
    } catch { response.writeHead(404).end(); }
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => new Promise(resolve => server.close(resolve)));
  const browser = await chromium.launch({ headless: true });
  t.after(() => browser.close());
  const fixtures = Object.fromEntries(await Promise.all(['commit', 'fragment'].map(async kind => [
    kind, (await readFile(new URL(`../../tests/fixtures/codec-${kind}-v1.hex`, import.meta.url), 'utf8')).trim(),
  ])));
  for (const entry of ['web', 'bundler', 'slimSync', 'slimAsync', 'iife']) {
    const page = await browser.newPage();
    try {
      await page.goto(`http://127.0.0.1:${server.address().port}/`);
      if (entry === 'iife') {
        await page.addScriptTag({ url: '/dist/wrapper/iife/index.js' });
        await page.evaluate(() => { globalThis.stockfoilIife = globalThis.WasmBodgeWrapper(); });
      }
      const result = await page.evaluate(async entry => {
        const api = entry === 'iife' ? globalThis.stockfoilIife : await import(`/dist/wrapper/esm/${entry.startsWith('slim') ? 'slim' : entry}.js`);
        if (entry === 'slimSync') api.initSync(await (await fetch('/dist/stockfoil.wasm')).arrayBuffer());
        if (entry === 'slimAsync') await api.default(new URL('/dist/stockfoil.wasm', location.href));
        const { CommitGraph, MaterializationCursor, ExportCursor, authorId, StockfoilError } = api;
        const { runsCodec } = await import('/test/runs-codec.mjs');
        const keys = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify']);
        const author = authorId(new Uint8Array(await crypto.subtle.exportKey('raw', keys.publicKey)));
        const source = new CommitGraph(runsCodec()), receiver = new CommitGraph(runsCodec());
        try {
          const draft = source.commit(author, 'browser');
          const signature = new Uint8Array(await crypto.subtle.sign('Ed25519', keys.privateKey, draft.signingBytes));
          source.addSignature({ author, commit: draft.hash, signature });
          for (const item of source.exportChanges(new ExportCursor()).added) receiver.ingest(item.bytes);
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
            payload: visible.changes[0].commit.payload,
            code,
          };
        } finally { source.dispose(); receiver.dispose(); }
      }, entry);
      assert.deepEqual(result, {
        admitted: true, denied: 'neither', visible: 'after', newlyDelivered: false,
        payload: 'browser', code: 'InvalidInput',
      }, entry);
      // Native golden envelopes, codec ownership and callback guards run through
      // every generated browser loader, in addition to permission transitions.
      await page.evaluate(async ({ entry, fixtures }) => {
        const api = entry === 'iife' ? globalThis.stockfoilIife : await import(`/dist/wrapper/esm/${entry.startsWith('slim') ? 'slim' : entry}.js`);
        const { runsCodec } = await import('/test/runs-codec.mjs');
        const { CommitGraph, MaterializationCursor, ExportCursor, authorId, StockfoilError } = api;
        const check = (condition, message) => { if (!condition) throw new Error(message); };
        const equal = (a, b, message) => check(JSON.stringify(a) === JSON.stringify(b), message);
        const throws = (fn, code, cause) => {
          try { fn(); } catch (error) {
            check(error instanceof StockfoilError && error.code === code, `expected ${code}, got ${error}`);
            if (cause !== undefined) check(error.cause === cause, 'exact callback cause');
            return;
          }
          throw new Error(`expected ${code}`);
        };
        const unhex = hex => Uint8Array.from(hex.match(/../g), b => parseInt(b, 16));
        const hex = bytes => Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('');
        // The same signer and complete envelopes as native tests/codec.rs.
        const key = await crypto.subtle.importKey('pkcs8', unhex('302e020100300506032b657004220420' + '07'.repeat(32)), 'Ed25519', true, ['sign']);
        const jwk = await crypto.subtle.exportKey('jwk', key);
        const author = authorId(Uint8Array.from(atob(jwk.x.replaceAll('-', '+').replaceAll('_', '/')), c => c.charCodeAt(0)));
        const sign = async (g, draft) => g.addSignature({ author, commit: draft.hash,
          signature: new Uint8Array(await crypto.subtle.sign('Ed25519', key, draft.signingBytes)) });
        for (const kind of ['commit', 'fragment']) {
          const g = new CommitGraph(runsCodec());
          try {
            const bytes = unhex(fixtures[kind]);
            g.setDecodeLimits({ maxItemBytes: 0 });
            throws(() => g.ingest(bytes), 'LimitExceeded');
            check(g.pendingItems() === 0, 'budget rejection is not queued');
            g.setDecodeLimits({});
            g.ingest(bytes);
            const batch = g.exportChanges(new ExportCursor());
            check(batch.added.length === 1 && batch.added[0].metadata.kind === kind, 'native envelope kind');
            check(hex(batch.added[0].bytes) === fixtures[kind], 'native/Wasm complete envelope equality');
            const changes = g.changes(new MaterializationCursor());
            const payloads = kind === 'commit' ? ['hello'] : [...Array(100).fill('shared long application key and repeated value'), 'checkpoint:129955'];
            equal(changes.changes.map(c => c.commit.payload), payloads, 'native payloads');
            let parents = [];
            for (const { commit } of changes.changes) {
              const expected = api.describeCommit(author, parents, new TextEncoder().encode(commit.payload));
              check(commit.hash === expected.hash, 'native payload/hash identity');
              parents = [commit.hash];
            }
            check(g.changes(changes.cursor.clone()).changes.length === 0, 'materialization cursor clone');
            check(g.exportChanges(batch.cursor.clone()).added.length === 0, 'export cursor clone');
          } finally { g.dispose(); }
        }
        const local = new CommitGraph(runsCodec(), { maxItemBytes: 0, maxPayloadBytes: 0 });
        try {
          const draft = local.commit(author, 'hello');
          await sign(local, draft);
          check(hex(local.exportChanges(new ExportCursor()).added[0].bytes) === fixtures.commit, 'WebCrypto seed7 singleton matches native golden');
        } finally { local.dispose(); }

        const encode = value => new TextEncoder().encode(JSON.stringify(value));
        const decode = bytes => JSON.parse(new TextDecoder().decode(bytes));
        let source, encoderAlias, decoderAlias, fail = false, reentries = 0;
        const failure = { message: 'browser callback failure' };
        const codec = {
          formatId: 'browser/json/v1',
          encodeCommit(value) {
            encoderAlias = value;
            if (source) {
              throws(() => source.viewPoint(), 'ReentrantCall');
              throws(() => source.dispose(), 'ReentrantCall');
              throws(() => source.setDecodeLimits({}), 'ReentrantCall');
              reentries++;
            }
            if (fail) throw failure;
            return encode(value);
          },
          decodeCommit(bytes) { decoderAlias = decode(bytes); return decoderAlias; },
          encodeFragment(_shape, values) { return encode(values); },
          decodeFragment(_shape, bytes) { decoderAlias = decode(bytes); return decoderAlias; },
        };
        source = new CommitGraph(codec);
        const receiver = new CommitGraph({ ...codec, encodeCommit: encode });
        const materialize = g => g.changes(new MaterializationCursor()).changes.map(c => c.commit.payload);
        try {
          const original = { nested: { value: 'browser-owned' } };
          const draft = source.commit(author, original);
          original.nested.value = 'caller mutation';
          encoderAlias.nested.value = 'encoder mutation';
          await sign(source, draft);
          const singleton = source.exportChanges(new ExportCursor()).added[0];
          receiver.ingest(singleton.bytes);
          decoderAlias.nested.value = 'decoder mutation';
          equal(materialize(receiver), [{ nested: { value: 'browser-owned' } }], 'owned decoded payload');
          const delivered = materialize(receiver);
          delivered[0].nested.value = 'reader mutation';
          equal(materialize(receiver), materialize(source), 'owned materialization');
          fail = true;
          throws(() => source.commit(author, { bad: true }), 'CodecError', failure);
          fail = false;
          check(materialize(source).length === 1, 'callback failure is atomic');
          // Mine a real depth-one checkpoint so primary browser fragment callbacks run.
          let checkpoint;
          for (let n = 0; n < 100000; n++) {
            const value = { checkpoint: n };
            const meta = source.describeCommit(author, value);
            if (meta.hash.startsWith('00') && !meta.hash.startsWith('0000')) { checkpoint = value; break; }
          }
          check(checkpoint !== undefined, 'checkpoint found');
          const head = source.commit(author, checkpoint);
          await sign(source, head);
          const fragment = source.exportChanges(new ExportCursor()).added.find(i => i.metadata.kind === 'fragment' && i.metadata.head === head.hash);
          check(fragment !== undefined, 'actual fragment export');
          receiver.ingest(fragment.bytes);
          decoderAlias[0].nested.value = 'fragment decoder mutation';
          equal(materialize(receiver), materialize(source), 'fragment roundtrip and clone isolation');
          check(reentries > 0 && receiver.hasCommit(head.hash), 'reentry recovery');
          const retained = singleton.bytes;
          source.dispose();
          check(retained.length > 0, 'owned export after disposal');
          throws(() => source.viewPoint(), 'Disposed');
        } finally { source.dispose(); receiver.dispose(); }
      }, { entry, fixtures });
    } finally { await page.close(); }
  }
});
