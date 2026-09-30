// Application BCS payloads only; Stockfoil owns its envelope and verification.
const DEFAULT_MAX_BYTES = 8 * 1024 * 1024;
const utf8 = new TextEncoder();
const text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
const defaults = { maxItemBytes: DEFAULT_MAX_BYTES, maxPayloadBytes: DEFAULT_MAX_BYTES, maxMembers: 65536 };
function check(ok) { if (!ok) throw new Error('invalid or oversized register payload'); }
function lengthSize(n) { let size = 1; while (n >= 128) { size++; n = Math.floor(n / 128); } return size; }
class Writer {
  parts = [];
  raw(b) { this.parts.push(b); }
  length(n) {
    check(Number.isSafeInteger(n) && n >= 0 && n <= 0xffffffff);
    do { const b = n % 128; n = Math.floor(n / 128); this.raw(Uint8Array.of(b | (n ? 128 : 0))); } while (n);
  }
  u32(n) { check(Number.isInteger(n) && n >= 0 && n <= 0xffffffff); const b = new Uint8Array(4); new DataView(b.buffer).setUint32(0, n, true); this.raw(b); }
  string(s) { check(typeof s === 'string' && s.isWellFormed()); const b = utf8.encode(s); this.length(b.length); this.raw(b); }
  finish() { const size = this.parts.reduce((n, b) => n + b.length, 0); check(Number.isSafeInteger(size)); const out = new Uint8Array(size); let i = 0; for (const b of this.parts) { out.set(b, i); i += b.length; } return out; }
}
class Reader {
  offset = 0;
  constructor(bytes, limits) { check(bytes instanceof Uint8Array && bytes.length <= limits.maxItemBytes); this.bytes = bytes; }
  raw(n) { check(Number.isSafeInteger(n) && n >= 0 && n <= this.bytes.length - this.offset); const b = this.bytes.subarray(this.offset, this.offset + n); this.offset += n; return b; }
  count(max) {
    let n = 0;
    for (let i = 0; i < 5; i++) { const b = this.raw(1)[0]; check(i !== 4 || b <= 15); n += (b & 127) * 2 ** (7 * i); if (!(b & 128)) { check((i === 0 || b !== 0) && n <= max); return n; } }
    throw new Error('invalid BCS length');
  }
  u32() { const b = this.raw(4); return new DataView(b.buffer, b.byteOffset, 4).getUint32(0, true); }
  string() { const n = this.count(this.bytes.length - this.offset); return { text: text.decode(this.raw(n)), size: n + lengthSize(n) }; }
  finish() { check(this.offset === this.bytes.length); }
}
export function encodeAssignment({ key, value }) { const w = new Writer(); w.string(key); w.string(value); return w.finish(); }
export function decodeAssignment(bytes, limits = defaults) {
  check(bytes.length <= limits.maxPayloadBytes);
  const r = new Reader(bytes, limits); const key = r.string().text, value = r.string().text; r.finish(); return { key, value };
}
export const registerCodec = {
  formatId: 'stockfoil/mv-register/v1',
  encodeCommit: encodeAssignment,
  decodeCommit: decodeAssignment,
  encodeFragment(shape, payloads) {
    check(payloads.length === shape.members.length);
    const keys = [], indices = new Map(), runs = [];
    for (const p of payloads) {
      if (!indices.has(p.key)) { indices.set(p.key, keys.length); keys.push(p.key); }
      const index = indices.get(p.key), last = runs.at(-1);
      if (last?.[0] === index) last[1]++; else runs.push([index, 1]);
    }
    const w = new Writer(); w.length(keys.length); for (const key of keys) w.string(key);
    w.length(runs.length); for (const [i, n] of runs) { w.u32(i); w.u32(n); }
    w.length(payloads.length); for (const p of payloads) w.string(p.value);
    return w.finish();
  },
  decodeFragment(shape, bytes, limits = defaults) {
    const n = shape.members.length;
    check(n <= limits.maxMembers && n <= Math.floor(limits.maxPayloadBytes / 2));
    const r = new Reader(bytes, limits), keys = [], unique = new Set();
    const keyCount = r.count(Math.min(n, bytes.length));
    for (let i = 0; i < keyCount; i++) { const key = r.string(); check(!unique.has(key.text)); unique.add(key.text); keys.push(key); }
    const runCount = r.count(Math.min(n, Math.floor((bytes.length - r.offset) / 8))), runs = [];
    let total = 0, budget = 0, seen = 0;
    for (let i = 0; i < runCount; i++) {
      const index = r.u32(), count = r.u32();
      check(index < keys.length && index <= seen && count > 0 && runs.at(-1)?.[0] !== index);
      if (index === seen) seen++;
      total += count; budget += keys[index].size * count;
      check(total <= n && Number.isSafeInteger(budget) && budget <= limits.maxPayloadBytes);
      runs.push([index, count]);
    }
    check(total === n && seen === keys.length && r.count(n) === n);
    const values = [];
    for (let i = 0; i < n; i++) { const value = r.string(); budget += value.size; check(budget <= limits.maxPayloadBytes); values.push(value.text); }
    r.finish();
    // Expand only after validating counts and the total canonical byte budget.
    const result = []; let offset = 0;
    for (const [index, count] of runs) for (let i = 0; i < count; i++) result.push({ key: keys[index].text, value: values[offset++] });
    return result;
  },
};
