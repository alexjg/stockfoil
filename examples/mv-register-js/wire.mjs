// Private example-only BCS v1 format; matches ../mv-register/src/wire.rs.
// Not a general BCS implementation or a stable Stockfoil storage format.
import { authorId, commitHash, describeCommit } from 'stockfoil';

const MAX_BYTES = 8 * 1024 * 1024;
const utf8 = new TextEncoder();
const text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
export const bytes = hex => {
  if (!/^[0-9a-f]{64}$/.test(hex)) throw new Error('expected 32-byte lowercase hex');
  return Uint8Array.from(Buffer.from(hex, 'hex'));
};

class Writer {
  parts = [];
  byte(n) { this.parts.push(Uint8Array.of(n)); }
  raw(value) { this.parts.push(value); }
  length(n) {
    if (!Number.isSafeInteger(n) || n < 0 || n > 0xffffffff) throw new Error('invalid BCS length');
    do { const digit = n % 128; n = Math.floor(n / 128); this.byte(digit | (n ? 128 : 0)); } while (n);
  }
  vector(values, write) { this.length(values.length); for (const value of values) write(value); }
  blob(value) { this.length(value.length); this.raw(value); }
  string(value) { this.blob(utf8.encode(value)); }
  finish() {
    const size = this.parts.reduce((sum, p) => sum + p.length, 0);
    if (size > MAX_BYTES) throw new Error('example blob exceeds size limit');
    const result = new Uint8Array(size);
    let offset = 0;
    for (const p of this.parts) { result.set(p, offset); offset += p.length; }
    return result;
  }
}
class Reader {
  offset = 0;
  constructor(input) {
    if (!(input instanceof Uint8Array) || input.length > MAX_BYTES) throw new Error('invalid example blob');
    this.input = input;
  }
  raw(n) {
    if (this.offset + n > this.input.length) throw new Error('truncated BCS');
    const result = this.input.slice(this.offset, this.offset + n);
    this.offset += n;
    return result;
  }
  byte() { return this.raw(1)[0]; }
  length() {
    let n = 0;
    for (let i = 0; i < 5; i++) {
      const b = this.byte();
      if (i === 4 && b > 15) throw new Error('overflowing BCS length');
      n += (b & 127) * 2 ** (7 * i);
      if (!(b & 128)) {
        if (i > 0 && b === 0) throw new Error('non-canonical BCS length');
        return n;
      }
    }
    throw new Error('invalid BCS length');
  }
  vector(read) {
    const n = this.length();
    // All vector elements in this private schema occupy at least one byte.
    if (n > this.input.length - this.offset) throw new Error('truncated BCS vector');
    return Array.from({ length: n }, read);
  }
  blob() { return this.raw(this.length()); }
  string() { return text.decode(this.blob()); }
  finish() { if (this.offset !== this.input.length) throw new Error('trailing BCS bytes'); }
}

export function encodeAssignment({ key, value }) {
  const w = new Writer(); w.string(key); w.string(value); return w.finish();
}
export function decodeAssignment(payload) {
  const r = new Reader(payload);
  const assignment = { key: r.string(), value: r.string() };
  r.finish(); return assignment;
}
export function encode(item) {
  const w = new Writer();
  const hash = h => w.raw(bytes(h));
  const commit = c => { hash(c.author); w.vector(c.parents, hash); w.blob(c.payload); };
  const evidence = s => { hash(s.author); hash(s.commit); w.blob(s.signature); };
  w.byte(1);
  if (item.kind === 'commit') {
    w.length(0); commit(item.commit);
  } else if (item.kind === 'fragment') {
    w.length(1); hash(item.head); w.vector(item.boundary, hash); w.vector(item.commits, commit);
  } else throw new Error('unknown item kind');
  w.vector(item.signatures, evidence);
  return w.finish();
}
export function decode(input) {
  const r = new Reader(input);
  if (r.byte() !== 1) throw new Error('unsupported example envelope version');
  const hash = () => commitHash(r.raw(32));
  const seen = new Set();
  const commit = () => {
    const author = authorId(r.raw(32));
    const parents = r.vector(hash);
    const c = describeCommit(author, parents, r.blob());
    if (seen.has(c.hash)) throw new Error('duplicate member');
    seen.add(c.hash);
    return c;
  };
  const evidence = () => {
    const author = authorId(r.raw(32)), commit = hash(), signature = r.blob();
    if (signature.length !== 64) throw new Error('signature must contain 64 bytes');
    return { author, commit, signature };
  };
  const tag = r.length();
  let item;
  if (tag === 0) item = { kind: 'commit', commit: commit() };
  else if (tag === 1) {
    const head = hash(), boundary = r.vector(hash);
    if (new Set(boundary).size !== boundary.length) throw new Error('duplicate boundary');
    item = { kind: 'fragment', head, boundary, commits: r.vector(commit) };
  } else throw new Error('unknown example item variant');
  item.signatures = r.vector(evidence);
  r.finish();
  // This only reconstructs metadata. CommitGraph.ingest verifies authenticity.
  return item;
}
