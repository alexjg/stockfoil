// Test-only byte codec and adversarial native-envelope encoder. This is NOT a
// graph adapter: tests instantiate the public CommitGraph and ingest bytes.
// Unlike production exports, fixtures may intentionally contain invalid evidence
// or incomplete fragment ranges. There is no structured transport production API.
const utf8 = new TextEncoder();
const text = new TextDecoder();
export const byteCodec = {
  formatId: 'test/bytes/v1',
  encodeCommit(value) {
    if (!(value instanceof Uint8Array)) throw new TypeError('expected bytes');
    return value;
  },
  decodeCommit: bytes => bytes,
  encodeFragment: (_shape, values) => utf8.encode(JSON.stringify(values.map(v => [...v]))),
  decodeFragment: (_shape, bytes) => JSON.parse(text.decode(bytes)).map(v => Uint8Array.from(v)),
};

function unsigned(value) {
  const out = [];
  do { const byte = value % 128; value = Math.floor(value / 128); out.push(byte | (value ? 128 : 0)); } while (value);
  return out;
}
const blob = bytes => [...unsigned(bytes.length), ...bytes];
function signed(value) {
  const out = [];
  for (;;) {
    const byte = value & 127;
    value = Math.floor(value / 128);
    const done = (value === 0 && !(byte & 64)) || (value === -1 && (byte & 64));
    out.push(byte | (done ? 0 : 128));
    if (done) return out;
  }
}
// Hexane canonical RLE: adjacent repeats coalesce; singleton runs are grouped
// into a negative-count literal span, not a count-one repeat.
function integers(values) {
  const out = [], literals = [];
  const flush = () => {
    if (literals.length) out.push(...signed(-literals.length), ...literals.flatMap(unsigned));
    literals.length = 0;
  };
  for (let i = 0; i < values.length;) {
    let end = i + 1;
    while (end < values.length && values[end] === values[i]) end++;
    if (end - i > 1) { flush(); out.push(...signed(end - i), ...unsigned(values[i])); }
    else literals.push(values[i]);
    i = end;
  }
  flush();
  return out;
}
function booleans(values) {
  if (!values.length) return [];
  const out = [];
  let current = false, count = 0;
  for (const value of values) {
    if (value !== current) { out.push(...unsigned(count)); current = value; count = 0; }
    count++;
  }
  out.push(...unsigned(count));
  return out;
}
const unhex = value => [...Buffer.from(value, 'hex')];

export function encodeFixture(item) {
  const fragment = item.kind === 'fragment';
  const pending = fragment ? [...item.commits] : [item.commit];
  const commits = [];
  // Preserve parent sequence and multiplicity; only order members topologically.
  while (pending.length) {
    let index = pending.findIndex(c => !c.parents.some(p => pending.some(other => other.hash === p)));
    if (index < 0) index = 0; // Let native validation reject a cyclic fixture.
    commits.push(...pending.splice(index, 1));
  }
  const indices = new Map(commits.map((c, i) => [c.hash, i]));
  const authors = [...new Set([...commits.map(c => c.author), ...item.signatures.map(s => s.author)])].sort();
  const external = [...new Set(commits.flatMap(c => c.parents).filter(p => !indices.has(p)))].sort();
  const parents = commits.flatMap((c, i) => c.parents.map(p => ({
    internal: indices.has(p), index: indices.has(p) ? i - indices.get(p) : external.indexOf(p),
  })));
  const boundary = fragment ? [...item.boundary].sort() : [];
  const body = fragment ? byteCodec.encodeFragment(null, commits.map(c => c.payload)) : item.commit.payload;
  return Uint8Array.from([
    ...utf8.encode('STKF'), 1, +fragment, ...blob(utf8.encode(byteCodec.formatId)),
    ...unhex(fragment ? item.head : item.commit.hash),
    ...unsigned(boundary.length), ...boundary.flatMap(unhex),
    ...unsigned(commits.length), ...unsigned(authors.length), ...authors.flatMap(unhex),
    ...unsigned(external.length), ...external.flatMap(unhex),
    ...blob(integers(commits.map(c => authors.indexOf(c.author)))),
    ...blob(integers(commits.map(c => c.parents.length))),
    ...blob(booleans(parents.map(p => p.internal))),
    ...blob(integers(parents.filter(p => p.internal).map(p => p.index))),
    ...blob(integers(parents.filter(p => !p.internal).map(p => p.index))),
    ...unsigned(item.signatures.length),
    ...blob(integers(item.signatures.map(s => authors.indexOf(s.author)))),
    ...blob(integers(item.signatures.map(s => indices.get(s.commit) ?? commits.length))),
    ...item.signatures.flatMap(s => [...s.signature]), ...blob(body),
  ]);
}
