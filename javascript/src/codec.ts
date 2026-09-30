import { StockfoilError, type PayloadCodec, type DecodeLimits, type FragmentShape } from "./types.js";

export const defaultLimits: DecodeLimits = Object.freeze({
  maxItemBytes: 8 * 1024 * 1024,
  maxMembers: 65536,
  maxParents: 1_000_000,
  maxSignatures: 65536,
  maxPayloadBytes: 8 * 1024 * 1024,
});

export function limits(options: Partial<DecodeLimits>): DecodeLimits {
  const result = { ...defaultLimits, ...options };
  for (const key of Object.keys(result) as (keyof DecodeLimits)[]) {
    if (!(key in defaultLimits) || !Number.isSafeInteger(result[key]) || result[key] < 0 || result[key] > 0xffffffff) {
      throw new StockfoilError("InvalidInput", `invalid decode limit: ${key}`);
    }
  }
  return Object.freeze(result);
}

/** structuredClone does not detach SharedArrayBuffer aliases, so reject them. */
function rejectShared(value: unknown, seen = new Set<object>()): void {
  if (typeof value !== "object" || value === null || seen.has(value)) return;
  seen.add(value);
  if (typeof SharedArrayBuffer !== "undefined" && value instanceof SharedArrayBuffer) {
    throw new TypeError("shared memory payloads are not supported");
  }
  if (value instanceof WebAssembly.Memory) rejectShared(value.buffer, seen);
  else if (ArrayBuffer.isView(value)) rejectShared(value.buffer, seen);
  else if (value instanceof Map) {
    for (const [key, entry] of value) { rejectShared(key, seen); rejectShared(entry, seen); }
  } else if (value instanceof Set) {
    for (const entry of value) rejectShared(entry, seen);
  } else {
    for (const entry of Object.values(value)) rejectShared(entry, seen);
    // structuredClone preserves Error.cause, which is non-enumerable.
    if (value instanceof Error) rejectShared(value.cause, seen);
  }
}

export function clonePayload<P>(value: P): P {
  const result = structuredClone(value);
  rejectShared(result);
  return result;
}

function synchronous<T>(value: T): T {
  if (value !== null && (typeof value === "object" || typeof value === "function") &&
      typeof (value as { then?: unknown }).then === "function") {
    throw new TypeError("payload codecs must be synchronous (no Promise or thenable results)");
  }
  return value;
}
function encoded(value: Uint8Array): Uint8Array {
  synchronous(value);
  if (!(value instanceof Uint8Array)) throw new TypeError("codec encoder must return Uint8Array");
  rejectShared(value);
  return new Uint8Array(value);
}

/** Capture the trusted codec configuration once; never select code from input. */
export function codecBridge<P>(codec: PayloadCodec<P>) {
  if (!codec || typeof codec.formatId !== "string" || !codec.formatId.length ||
      new TextEncoder().encode(codec.formatId).length > 128) {
    throw new StockfoilError("InvalidInput", "codec formatId must contain 1–128 UTF-8 bytes");
  }
  const formatId = codec.formatId;
  const { encodeCommit, decodeCommit, encodeFragment, decodeFragment } = codec;
  if ([encodeCommit, decodeCommit, encodeFragment, decodeFragment].some(fn => typeof fn !== "function")) {
    throw new StockfoilError("InvalidInput", "all four synchronous payload codec methods are required");
  }
  return {
    formatId,
    clonePayload,
    encodeCommit(payload: P) { return encoded(encodeCommit.call(codec, clonePayload(payload))); },
    decodeCommit(bytes: Uint8Array, budget: DecodeLimits) {
      return clonePayload(synchronous(decodeCommit.call(codec, bytes, budget)));
    },
    encodeFragment(shape: FragmentShape, payloads: readonly P[]) {
      return encoded(encodeFragment.call(codec, shape, clonePayload(payloads)));
    },
    decodeFragment(shape: FragmentShape, bytes: Uint8Array, budget: DecodeLimits) {
      const count = shape.members.length;
      const result = synchronous(decodeFragment.call(codec, shape, bytes, budget));
      if (!Array.isArray(result) || result.length !== count) {
        throw new TypeError("decodeFragment must return one payload per member");
      }
      return clonePayload(result);
    },
  };
}
