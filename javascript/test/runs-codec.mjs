// Exact test/runs/v1 semantics from tests/codec.rs (not a Stockfoil envelope).
export function runsCodec() {
  const utf8 = new TextEncoder();
  const text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
  return {
    formatId: 'test/runs/v1',
    encodeCommit: value => utf8.encode(value),
    decodeCommit: bytes => text.decode(bytes),
    encodeFragment(_shape, payloads) {
      const output = [];
      for (let i = 0; i < payloads.length;) {
        let end = i + 1;
        while (end < payloads.length && payloads[end] === payloads[i]) end++;
        const bytes = utf8.encode(payloads[i]);
        const header = new Uint8Array(8);
        const view = new DataView(header.buffer);
        view.setUint32(0, end - i, true);
        view.setUint32(4, bytes.length, true);
        output.push(...header, ...bytes);
        i = end;
      }
      return new Uint8Array(output);
    },
    decodeFragment(shape, bytes, limits) {
      const output = [];
      const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      let offset = 0, total = 0;
      while (offset < bytes.length) {
        if (bytes.length - offset < 8) throw new Error('truncated run');
        const count = view.getUint32(offset, true), length = view.getUint32(offset + 4, true);
        offset += 8;
        if (count === 0 || count > shape.members.length - output.length || length > bytes.length - offset) {
          throw new Error('invalid run');
        }
        total += count * length;
        if (!Number.isSafeInteger(total) || total > limits.maxPayloadBytes) throw new Error('payload budget');
        const value = text.decode(bytes.subarray(offset, offset + length));
        for (let i = 0; i < count; i++) output.push(value);
        offset += length;
      }
      return output;
    },
  };
}
