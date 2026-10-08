// Register assignments and the bundle codec Stockfoil uses to move them.
//
// Canonical payload (what a commit hash covers): UTF-8 JSON `[key, value]`.
// Encoded body: for each member, a u32 little-endian length and its canonical
// bytes. The decoder streams with bounded state, as the codec contract requires.

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export const encodeAssignment = (key, value) => encoder.encode(JSON.stringify([key, value]));
export function decodeAssignment(bytes) {
  const [key, value] = JSON.parse(decoder.decode(bytes));
  return { key, value };
}

function concat(parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) { out.set(p, offset); offset += p.length; }
  return out;
}

class LengthPrefixedDecoder {
  constructor(members) {
    this.members = members;
    this.ended = 0;
    this.prefix = new Uint8Array(4);
    this.have = 0; // prefix bytes collected for the current member
    this.remaining = -1; // payload bytes left in the current member, -1 before its prefix
  }

  step(input, eof, capacity) {
    let consumed = 0;
    const output = [];
    let produced = 0;
    const result = (status) => ({ consumed, output: concat(output), status });
    for (;;) {
      if (this.ended === this.members) {
        if (consumed < input.length) throw new Error('bytes after the last member');
        return result('finished');
      }
      if (this.remaining < 0) {
        const n = Math.min(4 - this.have, input.length - consumed);
        this.prefix.set(input.subarray(consumed, consumed + n), this.have);
        this.have += n;
        consumed += n;
        if (this.have === 4) {
          this.remaining = new DataView(this.prefix.buffer).getUint32(0, true);
          this.have = 0;
        }
      }
      if (this.remaining >= 0) {
        const n = Math.min(this.remaining, input.length - consumed, capacity - produced);
        output.push(input.subarray(consumed, consumed + n));
        consumed += n;
        produced += n;
        this.remaining -= n;
        if (this.remaining === 0) {
          this.remaining = -1;
          this.ended++;
          return result('endMember');
        }
        if (produced === capacity && produced > 0) return result('working');
      }
      if (consumed === input.length) {
        if (eof) throw new Error('truncated body');
        return result('needInput');
      }
    }
  }
}

export const registerCodec = {
  format: 'stockfoil-example/mv-register-v1',
  decoder: (meta) => new LengthPrefixedDecoder(meta.members.length),
  encode(_meta, payloads) {
    const parts = [];
    for (const payload of payloads) {
      const length = new Uint8Array(4);
      new DataView(length.buffer).setUint32(0, payload.length, true);
      parts.push(length, payload);
    }
    return concat(parts);
  },
};
