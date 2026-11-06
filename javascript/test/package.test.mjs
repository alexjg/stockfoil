import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import test from 'node:test';

test('CommonJS uses the same public facade, not generated Wasm classes', () => {
  execFileSync(process.execPath, ['-e', `
    const assert = require('node:assert/strict');
    const api = require('stockfoil');
    assert.equal(api.LedgerEngine, undefined);
    assert.equal(api.History, undefined);
    const ledger = new api.Ledger({ author: new Uint8Array(32), format: 'test' });
    assert.equal(ledger.length, 0);
    assert.equal(Object.getPrototypeOf(ledger.view()), Object.prototype);
    ledger.dispose();
  `], { cwd: new URL('../', import.meta.url), stdio: 'pipe' });
});

test('slim supports explicit initialization without exposing raw bindings in public types', () => {
  execFileSync(process.execPath, ['--input-type=module', '-e', `
    import assert from 'node:assert/strict';
    import { readFileSync } from 'node:fs';
    import { initialize, Ledger } from 'stockfoil/slim';
    initialize(readFileSync(new URL(import.meta.resolve('stockfoil/wasm'))));
    const ledger = new Ledger({ author: new Uint8Array(32), format: 'test' });
    assert.equal(ledger.length, 0);
    ledger.dispose();
  `], { cwd: new URL('../', import.meta.url), stdio: 'pipe' });
  const types = readFileSync(new URL('../dist/wrapper/index.d.ts', import.meta.url), 'utf8');
  assert.doesNotMatch(types, /#wasm-bodge|RawAncestry|Engine/);
});
