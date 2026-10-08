import { createPrivateKey, createPublicKey, sign } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';
import { ExportCursor, MemoryStorage } from 'stockfoil';

export const text = (s) => new TextEncoder().encode(s);
export const fromHex = (s) => Uint8Array.from(Buffer.from(s, 'hex'));

/** Ed25519 key from a 32-byte seed of `seed` bytes, matching the Rust tests' `key(seed)`. */
export function signer(seed) {
  const privateKey = createPrivateKey({
    key: Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), Buffer.alloc(32, seed)]),
    format: 'der', type: 'pkcs8',
  });
  const spki = createPublicKey(privateKey).export({ format: 'der', type: 'spki' });
  return {
    author: new Uint8Array(spki.subarray(-32)),
    sign: (hash) => new Uint8Array(sign(null, hash, privateKey)),
  };
}

export const alice = signer(1);
export const bob = signer(2);

function concat(parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) { out.set(p, offset); offset += p.length; }
  return out;
}

/** The core test codec: per member, `(u32 LE length, bytes)*` ending with a zero length. */
class ChunkDecoder {
  constructor(members) {
    this.members = members; this.ended = 0;
    this.length = new Uint8Array(4); this.have = 0;
    this.remaining = 0; this.inChunk = false;
  }
  step(input, eof, capacity) {
    let consumed = 0, produced = 0;
    const output = [];
    const done = (status) => ({ consumed, output: concat(output), status });
    for (;;) {
      if (this.ended === this.members) {
        if (consumed < input.length) throw new Error('trailing body bytes');
        return done('finished');
      }
      if (this.inChunk) {
        const n = Math.min(this.remaining, input.length - consumed, capacity - produced);
        output.push(input.subarray(consumed, consumed + n));
        consumed += n; produced += n; this.remaining -= n;
        if (this.remaining === 0) { this.inChunk = false; continue; }
        if (produced === capacity) return done('working');
      } else {
        const n = Math.min(4 - this.have, input.length - consumed);
        this.length.set(input.subarray(consumed, consumed + n), this.have);
        this.have += n; consumed += n;
        if (this.have === 4) {
          this.have = 0;
          const len = new DataView(this.length.buffer).getUint32(0, true);
          if (len === 0) { this.ended++; return done('endMember'); }
          this.remaining = len; this.inChunk = true;
          continue;
        }
      }
      if (consumed === input.length) {
        if (eof) throw new Error('truncated body');
        return done('needInput');
      }
    }
  }
}

export const chunkCodec = {
  format: 'stockfoil-test/chunks-v1',
  decoder: (meta) => new ChunkDecoder(meta.members.length),
  encode(_meta, payloads) {
    const parts = [];
    for (const payload of payloads) {
      for (let i = 0; i < payload.length; i += 4096) {
        const chunk = payload.subarray(i, i + 4096);
        const len = new Uint8Array(4);
        new DataView(len.buffer).setUint32(0, chunk.length, true);
        parts.push(len, chunk);
      }
      parts.push(new Uint8Array(4));
    }
    return concat(parts);
  },
};

/** Commit and sign; returns the hash. */
export async function signed(graph, who, parents, payload) {
  const prepared = await graph.commit(who.author, parents, payload);
  graph.sign(who.author, prepared.hash, who.sign(prepared.signingBytes));
  return prepared.hash;
}

/** Every planned item of `from` relative to `cursor`, as wire bytes. */
export async function items(from, cursor = new ExportCursor()) {
  const plan = from.export(cursor);
  const out = [];
  for (let i = 0; i < plan.length; i++) out.push(await from.writeItem(plan, i));
  return { plan, items: out };
}

/** Send everything `to` lacks, in plan order; returns the advanced cursor. */
export async function sync(from, to, cursor = new ExportCursor()) {
  const { plan, items: bytes } = await items(from, cursor);
  for (let i = 0; i < bytes.length; i++) {
    await to.receive(bytes[i]);
    plan.acknowledge(i);
  }
  return plan.cursor();
}

/** MemoryStorage whose reads and binds complete after random delays. */
export class SlowStorage extends MemoryStorage {
  reads = 0;
  async read(blob, offset, max) {
    this.reads++;
    await sleep(Math.random() * 3);
    return super.read(blob, offset, max);
  }
  async bind(location) {
    await sleep(Math.random() * 3);
    return super.bind(location);
  }
}

/** A depth-1 payload: its commit hash starts with a zero byte. */
export function payloadAtDepth(history, who, parents, tag) {
  for (let i = 0; ; i++) {
    const payload = text(`${tag}-${i}`);
    const builder = history.beginCommit(who.author, parents);
    builder.update(payload);
    if (builder.finish().hash[0] === 0) return payload;
  }
}

export const settled = async (promise) => {
  let state = 'pending';
  promise.then(() => { state = 'fulfilled'; }, () => { state = 'rejected'; });
  await sleep(20);
  return state;
};
