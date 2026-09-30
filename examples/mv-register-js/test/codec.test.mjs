// Pure payload tests: no Wasm build or network needed. Rust consumes the same
// golden vectors; each prefix is followed by exactly valueBytes ASCII 'x' bytes.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { registerCodec, encodeAssignment, decodeAssignment } from '../codec.mjs';

const vectors = JSON.parse(readFileSync(new URL('./fixtures/codec-boundaries.json', import.meta.url), 'utf8'));
const defaultMax = 8 * 1024 * 1024;
function golden(prefix, count) {
  return Buffer.concat([Buffer.from(prefix, 'hex'), Buffer.alloc(count, 0x78)]);
}

for (const { valueBytes, commitPrefix, fragmentPrefix } of vectors) {
  test(`Rust/JS golden canonical encoding with ${valueBytes} value bytes`, () => {
    const payload = { key: 'k', value: 'x'.repeat(valueBytes) };
    const shape = { members: [{}] };
    const commit = encodeAssignment(payload);
    const fragment = registerCodec.encodeFragment(shape, [payload]);
    assert.deepEqual(Buffer.from(commit), golden(commitPrefix, valueBytes));
    assert.deepEqual(Buffer.from(fragment), golden(fragmentPrefix, valueBytes));

    // Encoding is independent of budgets, and larger inbound budgets work.
    const limits = { maxItemBytes: fragment.length, maxPayloadBytes: commit.length, maxMembers: 1 };
    assert.deepEqual(decodeAssignment(commit, limits), payload);
    assert.deepEqual(registerCodec.decodeFragment(shape, fragment, limits), [payload]);
    for (const field of ['maxItemBytes', 'maxPayloadBytes']) {
      const tightCommit = { ...limits, [field]: commit.length - 1 };
      assert.throws(() => decodeAssignment(commit, tightCommit));
      const tightFragment = { ...limits, [field]: (field === 'maxItemBytes' ? fragment.length : commit.length) - 1 };
      assert.throws(() => registerCodec.decodeFragment(shape, fragment, tightFragment));
    }
    if (commit.length > defaultMax) assert.throws(() => decodeAssignment(commit));
    else assert.deepEqual(decodeAssignment(commit), payload);
    assert.throws(() => registerCodec.decodeFragment(shape, fragment));
  });
}

test('encoders still reject noncanonical strings and mismatched fragment shape', () => {
  assert.throws(() => encodeAssignment({ key: '\uD800', value: '' }));
  assert.throws(() => registerCodec.encodeFragment({ members: [{}] }, []));
  assert.throws(() => registerCodec.encodeFragment({ members: [{}] }, [{ key: 'k', value: '\uDC00' }]));
});
