/**
 * Stockfoil for JavaScript: the Wasm core objects plus the host loop that
 * drives an `Admission` against asynchronous JS storage and a JS codec.
 *
 * Every value returned by the core is an owned copy, and the driver holds
 * nothing borrowed across an `await`, so any number of admissions can be in
 * flight at once, interleaved with local commits on the same `History`.
 */
import {
  Admission, Changes, CommitBuilder, ExportCursor, ExportPlan, History,
  MaterializationCursor, Prepared, ViewPoint, blobHash, readHeader,
} from "#wasm-bodge/bindings";

export {
  Admission, Changes, CommitBuilder, ExportCursor, ExportPlan, History,
  MaterializationCursor, Prepared, ViewPoint, blobHash, readHeader,
};

// ---- shapes of the owned values the core returns ----

/** A 32-byte commit hash, blob hash, or author; or a 64-byte signature. */
export type Bytes = Uint8Array;
/** Application payload ID: always a BigInt, never a lossy number. */
export type PayloadId = bigint;

export type ItemId =
  | { kind: "commit"; head: Bytes }
  | { kind: "fragment"; head: Bytes; boundary: Bytes[] };

/** Untrusted until admitted. A numeric parent is an earlier member's index. */
export interface BundleMeta {
  id: ItemId;
  members: { author: Bytes; parents: (number | Bytes)[] }[];
  evidence: { author: Bytes; member: number; signature: Bytes }[];
}

export interface Delivered { hash: Bytes; payload: PayloadId; newlyDelivered: boolean }
export interface Receipt { id: ItemId; members: Delivered[]; draftsAdmitted: ItemId[] }

export type DecodeStatus = "working" | "needInput" | "endMember" | "finished";
export interface DecodeStep { consumed: number; output: Uint8Array; status: DecodeStatus }

export type Request =
  | { kind: "read"; item: Bytes; offset: number; max: number }
  | { kind: "decode"; input: Uint8Array; eof: boolean; capacity: number }
  | { kind: "bind"; item: Bytes; memberCount: number };

export type Next =
  | { type: "continue" }
  | { type: "request"; ticket: bigint; request: Request }
  | { type: "awaitReply" }
  | { type: "needHistory" }
  | { type: "waiting"; hash: Bytes }
  | { type: "done"; receipt: Receipt }
  | { type: "failed"; error: StockfoilError };

export type Reply =
  | { kind: "read"; bytes: Uint8Array; eof: boolean }
  | ({ kind: "decoded" } & DecodeStep)
  | { kind: "bound"; ids: readonly PayloadId[] | BigUint64Array }
  | { kind: "failed"; message: string };

export type Visibility = "before" | "after" | "both" | "neither";
export interface ChangeRecord {
  hash: Bytes; payload: PayloadId; newlyDelivered: boolean; visibility: Visibility;
}
export interface ChangeBatch {
  records: ChangeRecord[]; complete: boolean; cursor: MaterializationCursor | undefined;
}

/** `"unbounded"`, `"denied"`, or a write frontier of commit hashes. */
export type WritePermission = "unbounded" | "denied" | Bytes[];
/** An iterable of `[author, permission]`, for example an array or a `Map`. */
export type WritePermissions = Iterable<readonly [Bytes, WritePermission]>;

export interface Limits {
  maxHeaderBytes?: number; maxMembers?: number; maxParents?: number;
  maxSignatures?: number; maxBoundaries?: number;
  maxCanonicalBytes?: number; maxItemBytes?: number;
}
export interface ReceiveOptions {
  format: string; readSize?: number; decodeCapacity?: number; quantum?: number; limits?: Limits;
}

/** Errors thrown by the core carry a `code` naming the failed rule. */
export interface StockfoilError extends Error {
  name: "StockfoilError";
  code: string;
  hash?: Bytes;
  author?: Bytes;
}

type MaybePromise<T> = T | Promise<T>;

// ---- the host loop ----

/** One decoder per admission, created from the item's metadata. */
export interface Decoder {
  /** Decode some of `input` into at most `capacity` canonical bytes. */
  step(input: Uint8Array, eof: boolean, capacity: number): MaybePromise<DecodeStep>;
}

/** What `drive` needs from the application. */
export interface Host {
  /** Up to `max` bytes of stored item `item` at `offset`; empty at its end. */
  read(item: Bytes, offset: number, max: number): MaybePromise<Uint8Array>;
  /** Allocate one payload ID per member of stored item `item`, in member order. */
  bind(item: Bytes, memberCount: number): MaybePromise<readonly PayloadId[]>;
  decoder(meta: BundleMeta): MaybePromise<Decoder>;
}

export interface DriveOptions {
  /** Abort a drive, including one waiting for a dependency. */
  signal?: AbortSignal;
}

type Schedule = (callback: () => void) => unknown;
const schedule: Schedule = (globalThis as { setImmediate?: Schedule }).setImmediate ?? setTimeout;
const turn = () => new Promise<void>(resolve => schedule(resolve));

function aborted(signal: AbortSignal): Promise<never> {
  return new Promise((_, reject) => {
    if (signal.aborted) reject(signal.reason);
    else signal.addEventListener("abort", () => reject(signal.reason), { once: true });
  });
}

/**
 * Drive one admission to completion: execute its requests against `host`,
 * advance it on `history`, and wait for missing dependencies, retrying each
 * time `history.epoch()` changes. Resolves with the receipt; rejects with the
 * operation's `StockfoilError` (its `cause` is the host error, if one caused it).
 */
export async function drive(
  history: History, admission: Admission, host: Host, options: DriveOptions = {},
): Promise<Receipt> {
  const { signal } = options;
  let decoder: Decoder | undefined;
  let hostError: unknown;
  for (;;) {
    signal?.throwIfAborted();
    let next = admission.next() as Next;
    if (next.type === "needHistory") next = history.advance(admission) as Next;
    switch (next.type) {
      case "continue":
        await turn(); // one work quantum done; let other operations run
        break;
      case "awaitReply": {
        // Only reachable if a request was issued but not answered here.
        next = { type: "request", ...admission.request() } as Next;
      }
      // falls through
      case "request": {
        const { ticket, request } = next as Extract<Next, { type: "request" }>;
        let reply: Reply;
        try {
          reply = await execute(request);
        } catch (error) {
          hostError = error;
          reply = { kind: "failed", message: String((error as Error)?.message ?? error) };
        }
        try {
          admission.reply(ticket, reply);
        } catch (error) {
          // A protocol violation has failed the operation; `next()` reports it.
          // A reply the binding could not read leaves it outstanding: fail it.
          if ((error as StockfoilError).code !== "InvalidInput") break;
          hostError = error;
          admission.reply(ticket, { kind: "failed", message: (error as Error).message });
        }
        break;
      }
      case "waiting": {
        const changed = history.epochChanged(history.epoch()) as Promise<void>;
        await (signal ? Promise.race([changed, aborted(signal)]) : changed);
        break;
      }
      case "needHistory":
        break;
      case "done":
        return next.receipt;
      case "failed":
        if (hostError !== undefined) next.error.cause = hostError;
        throw next.error;
    }
  }

  async function execute(request: Request): Promise<Reply> {
    switch (request.kind) {
      case "read": {
        const bytes = await host.read(request.item, request.offset, request.max);
        return { kind: "read", bytes, eof: bytes.length === 0 };
      }
      case "decode": {
        decoder ??= await host.decoder(admission.meta() as BundleMeta);
        const step = await decoder.step(request.input, request.eof, request.capacity);
        return { kind: "decoded", ...step };
      }
      case "bind":
        return { kind: "bound", ids: await host.bind(request.item, request.memberCount) };
    }
  }
}

// ---- convenience graph: history + JS codec + JS storage ----

export type PayloadLocation =
  | { kind: "canonical"; blob: Bytes }
  | { kind: "member"; item: Bytes; index: number };

/**
 * Content-addressed blobs and payload IDs. `put` returns the BLAKE3 hash of
 * the bytes (see `blobHash`). IDs from `bind` must never be rebound.
 */
export interface Storage {
  put(bytes: Uint8Array): MaybePromise<Bytes>;
  read(blob: Bytes, offset: number, max: number): MaybePromise<Uint8Array>;
  bind(location: PayloadLocation): MaybePromise<PayloadId>;
  location(id: PayloadId): MaybePromise<PayloadLocation>;
}

/** An application bundle codec. `encode` produces a whole body. */
export interface Codec {
  format: string;
  decoder(meta: BundleMeta): MaybePromise<Decoder>;
  encode(meta: BundleMeta, payloads: Uint8Array[]): MaybePromise<Uint8Array>;
}

export const hex = (bytes: Uint8Array): string =>
  Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");

/** In-memory `Storage`. `firstId` sets the first payload ID handed out. */
export class MemoryStorage implements Storage {
  readonly blobs = new Map<string, Uint8Array>();
  readonly locations = new Map<PayloadId, PayloadLocation>();
  private nextId: PayloadId;
  constructor(options: { firstId?: PayloadId } = {}) {
    this.nextId = options.firstId ?? 0n;
  }
  put(bytes: Uint8Array): Bytes {
    const hash = blobHash(bytes);
    this.blobs.set(hex(hash), bytes.slice());
    return hash;
  }
  read(blob: Bytes, offset: number, max: number): Uint8Array {
    const bytes = this.blobs.get(hex(blob));
    if (!bytes) throw new Error(`unknown blob ${hex(blob)}`);
    return bytes.slice(offset, offset + max);
  }
  bind(location: PayloadLocation): PayloadId {
    const id = this.nextId++;
    this.locations.set(id, location);
    return id;
  }
  location(id: PayloadId): PayloadLocation {
    const location = this.locations.get(id);
    if (!location) throw new Error(`unknown payload ${id}`);
    return location;
  }
}

const CHUNK = 64 * 1024;

function concat(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) { out.set(p, offset); offset += p.length; }
  return out;
}

/**
 * A `History` with a JS codec and storage. Methods may run concurrently;
 * `receive` waits for missing dependencies (pass a `signal` to give up).
 */
export class Graph {
  readonly history = new History();
  readonly host: Host;
  constructor(
    readonly codec: Codec,
    readonly storage: Storage = new MemoryStorage(),
    readonly options: Omit<ReceiveOptions, "format"> = {},
  ) {
    this.host = {
      read: (item, offset, max) => storage.read(item, offset, max),
      bind: async (item, memberCount) => {
        const ids: PayloadId[] = [];
        for (let index = 0; index < memberCount; index++) {
          ids.push(await storage.bind({ kind: "member", item, index }));
        }
        return ids;
      },
      decoder: meta => codec.decoder(meta),
    };
  }

  /** Store an incoming item (header and body), then admit it. */
  async receive(item: Uint8Array, options?: DriveOptions): Promise<Receipt> {
    return this.admit(await this.storage.put(item), options);
  }

  /** Admit an item already in storage. */
  async admit(item: Bytes, options?: DriveOptions): Promise<Receipt> {
    const admission = this.history.receive(item, { ...this.options, format: this.codec.format });
    try {
      return await drive(this.history, admission, this.host, options);
    } finally {
      admission.free();
    }
  }

  /** Store and deliver a local draft. Sign `signingBytes`, then call `sign`. */
  async commit(author: Bytes, parents: Bytes[], payload: Uint8Array): Promise<Prepared> {
    const builder = this.history.beginCommit(author, parents);
    builder.update(payload);
    const prepared = builder.finish();
    const blob = await this.storage.put(payload);
    const id = await this.storage.bind({ kind: "canonical", blob });
    this.history.publishCommit(prepared, id);
    return prepared;
  }

  /** Attach a signature to a draft. Returns the item IDs this admitted. */
  sign(author: Bytes, hash: Bytes, signature: Bytes): ItemId[] {
    return this.history.sign(author, hash, signature) as ItemId[];
  }

  export(cursor: ExportCursor = new ExportCursor()): ExportPlan {
    return this.history.export(cursor);
  }

  /** Planned item `index` as wire bytes: header followed by the encoded body. */
  async writeItem(plan: ExportPlan, index: number): Promise<Uint8Array> {
    const item = this.history.exportItem(plan, index, this.codec.format) as {
      header: Uint8Array; members: { payload: PayloadId }[]; meta: BundleMeta;
    };
    const payloads = await this.payloads(item.members.map(m => m.payload));
    return concat([item.header, await this.codec.encode(item.meta, payloads)]);
  }

  /** One payload's canonical bytes. */
  async payload(id: PayloadId): Promise<Uint8Array> {
    return (await this.payloads([id]))[0];
  }

  /** Canonical bytes for several payloads, decoding each stored item once. */
  async payloads(ids: readonly PayloadId[]): Promise<Uint8Array[]> {
    const items = new Map<string, Promise<Uint8Array[]>>();
    return Promise.all(ids.map(async id => {
      const location = await this.storage.location(id);
      if (location.kind === "canonical") return this.readAll(location.blob);
      const key = hex(location.item);
      if (!items.has(key)) items.set(key, this.decodeItem(location.item));
      return (await items.get(key)!)[location.index];
    }));
  }

  private async readAll(blob: Bytes): Promise<Uint8Array> {
    const parts: Uint8Array[] = [];
    for (let offset = 0; ;) {
      const part = await this.storage.read(blob, offset, CHUNK);
      if (part.length === 0) return concat(parts);
      parts.push(part);
      offset += part.length;
    }
  }

  /** Every member's canonical bytes from a stored item. */
  async decodeItem(item: Bytes): Promise<Uint8Array[]> {
    const bytes = await this.readAll(item);
    const header = readHeader(bytes, this.codec.format, this.options.limits) as
      { length: number; meta: BundleMeta } | undefined;
    if (!header) throw new Error("truncated item header");
    const decoder = await this.codec.decoder(header.meta);
    const members: Uint8Array[] = [];
    let current: Uint8Array[] = [];
    let input = bytes.subarray(header.length);
    for (;;) {
      const step = await decoder.step(input, true, CHUNK);
      input = input.subarray(step.consumed);
      current.push(step.output);
      if (step.status === "endMember") { members.push(concat(current)); current = []; }
      if (step.status === "finished") return members;
      if (step.status === "needInput") throw new Error("decoder needs input after the body");
    }
  }
}
