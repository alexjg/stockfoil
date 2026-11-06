/**
 * Transport-neutral ledger and materialization engines.
 * Data crossing Wasm is copied, owned and structured-cloneable. Only stateful
 * engines live in Wasm; none of their generated classes are publicly exported.
 */
import {
  LedgerEngine, MaterializationEngine,
  encodeExportCursor as encodeCursor, decodeExportCursor as decodeCursor,
  inspectCheckpoint as inspect, initSync,
  type Admission as RawAdmission, type CommitBuilder as RawBuilder,
  type DecodeItem as RawDecode, type Ancestry as RawAncestry,
} from "#wasm-bodge/bindings";

/** Manual initialization for stockfoil/slim. Root imports initialize automatically. */
export function initialize(module: BufferSource | WebAssembly.Module): void { initSync({ module }); }

export type Bytes = Uint8Array;
export type PayloadId = bigint;
export type ItemId =
  | { kind: "commit"; head: Bytes }
  | { kind: "fragment"; head: Bytes; boundary: Bytes[] };
export interface BundleMeta {
  id: ItemId;
  members: { author: Bytes; parents: (number | Bytes)[] }[];
}
export interface Delivered { hash: Bytes; payload: PayloadId; newlyDelivered: boolean }
export interface Receipt { id: ItemId; members: Delivered[]; draftsAdmitted: ItemId[] }
export interface Failure { id: ItemId; error: StockfoilError }
export interface Signed { admitted: Bytes[]; released: Receipt[]; failed: Failure[] }
export type Received =
  | { kind: "admitted"; items: Receipt[]; delivered: Bytes[]; failed: Failure[] }
  | { kind: "waiting"; waiting: Bytes }
  | { kind: "evicted"; waiting: Bytes; evicted: ItemId };
export interface Draft { hash: Bytes; author: Bytes; parents: Bytes[] }
export interface CommitInfo { payload: PayloadId; author: Bytes; admitted: boolean }
export type WritePermission = "unbounded" | "denied" | Bytes[];
export type WritePermissions = Iterable<readonly [Bytes, WritePermission]>;
export type Follow = "working" | "signed" | { kind: "pinned"; heads: Bytes[] };
export interface View { heads: Bytes[]; permissions: [Bytes, WritePermission][] }
export interface Limits {
  maxHeaderBytes?: number; maxMembers?: number; maxParents?: number;
  maxSignatures?: number; maxBoundaries?: number;
  maxCanonicalBytes?: number; maxItemBytes?: number;
}
export interface ReceiveOptions {
  readSize?: number; decodeCapacity?: number; quantum?: number; limits?: Limits;
}
export interface LedgerOptions extends ReceiveOptions {
  author: Bytes; format: string; permissions?: WritePermissions; maxParked?: number;
}
export interface StockfoilError extends Error {
  name: "StockfoilError"; code: string; hash?: Bytes; author?: Bytes;
}
export type DecodeStatus = "working" | "needInput" | "endMember" | "finished";
export interface DecodeStep { consumed: number; output: Uint8Array; status: DecodeStatus }
export type Request =
  | { kind: "read"; offset: number; max: number }
  | { kind: "decode"; input: Uint8Array; eof: boolean; capacity: number }
  | { kind: "bind"; memberCount: number };
export type Next =
  | { type: "continue" }
  | { type: "request"; ticket: bigint; request: Request }
  | { type: "awaitReply" }
  | { type: "needHistory" }
  | { type: "waiting"; hash: Bytes }
  | { type: "done"; receipt: Receipt }
  | { type: "failed"; error: StockfoilError };
export type DecodeNext =
  | Extract<Next, { type: "request" | "awaitReply" | "failed" }>
  | { type: "chunk"; member: number; bytes: Bytes }
  | { type: "done" };
export type Reply =
  | { kind: "read"; bytes: Bytes; eof: boolean }
  | ({ kind: "decoded" } & DecodeStep)
  | { kind: "bound"; ids: readonly PayloadId[] | BigUint64Array }
  | { kind: "failed"; message: string };
export interface Counts {
  bytesRead: number; encodedBytesDecoded: number; canonicalBytes: number; requests: number;
}
type MaybePromise<T> = T | Promise<T>;
/** Return staged IDs synchronously; do not reenter the ledger from this callback. */
export type Bind = (item: ItemId, members: Bytes[]) => readonly PayloadId[];

function resource<T extends { free(): void }>(value: T) {
  let live: T | undefined = value;
  return {
    get(): T { if (!live) throw new Error("Stockfoil resource is disposed"); return live; },
    take(): T { const value = this.get(); live = undefined; return value; },
    dispose() { const value = live; live = undefined; value?.free(); },
  };
}
type Resource<T extends { free(): void }> = ReturnType<typeof resource<T>>;

export interface Operation {
  reply(ticket: bigint, reply: Reply): void;
  request(): Extract<Next, { type: "request" }> | undefined;
  meta(): BundleMeta | undefined;
  counts(): Counts;
  dispose(): void;
}
export interface Admission extends Operation { next(): Next }
export interface DecodeItem extends Operation { next(): DecodeNext }
export interface CommitBuilder { update(bytes: Bytes): void; hash(): Bytes; dispose(): void }
const admissions = new WeakMap<Admission, Resource<RawAdmission>>();
const builders = new WeakMap<CommitBuilder, Resource<RawBuilder>>();
const ledgers = new WeakMap<Ledger, Resource<LedgerEngine>>();
function native(ledger: Ledger): LedgerEngine { return ledgers.get(ledger)!.get(); }
function operation<T extends RawAdmission | RawDecode>(value: T) {
  const r = resource(value);
  return { r, facade: {
    next: () => r.get().next(),
    reply: (ticket: bigint, reply: Reply) => r.get().reply(ticket, reply),
    request: () => r.get().request(),
    meta: () => r.get().meta(),
    counts: () => r.get().counts(),
    dispose: () => r.dispose(),
  } };
}

/** One document's topology, document-level policy and capped parking queue. No I/O. */
export class Ledger {
  readonly #options: { format: string } & ReceiveOptions;
  constructor(options: LedgerOptions) {
    ledgers.set(this, resource(new LedgerEngine(options)));
    this.#options = { format: options.format, readSize: options.readSize,
      decodeCapacity: options.decodeCapacity, quantum: options.quantum,
      limits: options.limits && { ...options.limits } };
  }
  dispose() { ledgers.get(this)!.dispose(); }
  get length(): number { return native(this).length; }
  get author(): Bytes { return native(this).author(); }
  get epoch(): bigint { return native(this).epoch(); }
  get parked(): number { return native(this).parked(); }
  heads(which: "delivered" | "admitted" = "delivered"): Bytes[] { return native(this).heads(which); }
  lookup(hash: Bytes): CommitInfo | undefined { return native(this).lookup(hash); }
  parents(hash: Bytes): Bytes[] | undefined { return native(this).parents(hash); }
  isAncestor(a: Bytes, d: Bytes): boolean { return native(this).isAncestor(a, d); }
  permissions(): [Bytes, WritePermission][] { return native(this).permissions(); }
  setPermissions(value: WritePermissions): void { native(this).setPermissions(value); }
  view(follow: Follow = "working"): View { return native(this).view(follow); }
  drafts(): Draft[] { return native(this).drafts(); }
  missing(): Bytes[] { return native(this).missing(); }
  beginCommit(parents?: Bytes[]): CommitBuilder {
    const r = resource(native(this).beginCommit(parents));
    const builder = { update: (bytes: Bytes) => r.get().update(bytes),
      hash: () => r.get().hash(), dispose: () => r.dispose() };
    builders.set(builder, r);
    return builder;
  }
  /** Consume a builder after storing its canonical bytes. Returns an ordinary draft object. */
  commit(builder: CommitBuilder, payload: PayloadId): Draft {
    // Validate before taking the consuming Wasm argument.
    if (typeof payload !== "bigint" || payload < 0n || payload >= 2n ** 64n)
      throw Object.assign(new Error("payload ID must be a u64 BigInt"), { code: "InvalidInput" });
    return native(this).commit(builders.get(builder)!.take(), payload);
  }
  attachSignature(draft: Draft, signature: Bytes, bind: Bind): Signed {
    return native(this).attachSignature(draft, signature, bind);
  }
  beginReceive(options: ReceiveOptions = {}): Admission {
    const { r, facade } = operation(native(this).beginReceive({ ...this.#options, ...options }));
    admissions.set(facade, r);
    return facade;
  }
  /** Consume a byte-verified admission. Missing dependencies park instead of waiting. */
  admit(admission: Admission, bind: Bind): Received {
    return native(this).admit(admissions.get(admission)!.take(), bind);
  }
  export(cursor: ExportCursor = []): ExportPlan { return native(this).export(cursor); }
  decodeItem(wanted: Iterable<readonly [number, Bytes]>, options: ReceiveOptions = {}): DecodeItem {
    return operation(native(this).decodeItem(wanted, { ...this.#options, ...options })).facade;
  }
  checkPayload(hash: Bytes) {
    const r = resource(native(this).checkPayload(hash));
    return { update: (bytes: Bytes) => r.get().update(bytes),
      finish: () => r.take().finish(), dispose: () => r.dispose() };
  }
  ancestry(heads: Bytes[]): Ancestry { return Ancestry.capture(this, heads); }
}

/** An owned causal selection. Queries borrow a ledger only for the call. */
export class Ancestry {
  readonly #r: Resource<RawAncestry>;
  private constructor(value: RawAncestry) { this.#r = resource(value); }
  static capture(ledger: Ledger, heads: Bytes[]): Ancestry {
    return new Ancestry(native(ledger).ancestry(heads));
  }
  contains(ledger: Ledger, hash: Bytes): boolean { return this.#r.get().contains(native(ledger), hash); }
  participates(ledger: Ledger, hash: Bytes): boolean { return this.#r.get().participates(native(ledger), hash); }
  /** Collect causal differences (ignores policy). Graph metadata, never payload bytes. */
  difference(other: Ancestry, ledger: Ledger): Bytes[] {
    return this.#r.get().difference(other.#r.get(), native(ledger));
  }
  dispose() { this.#r.dispose(); }
}

/** An acknowledged inventory: plain data, not a Wasm handle. */
export type ExportCursor = ItemId[];
export interface ExportedItem {
  id: ItemId; header: Bytes; members: { hash: Bytes; payload: PayloadId }[];
  checkpoints: Bytes[]; meta: BundleMeta;
}
export interface ExportPlan { items: ExportedItem[]; retired: ItemId[]; cursor: ExportCursor }
export const encodeExportCursor = (cursor: ExportCursor): Bytes => encodeCursor(cursor);
export const decodeExportCursor = (bytes: Bytes): ExportCursor => decodeCursor(bytes);
export const hex = (bytes: Bytes): string => Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");
function itemKey(id: ItemId): string {
  return id.kind + ":" + hex(id.head) +
    (id.kind === "fragment" ? ":" + id.boundary.map(hex).sort().join(",") : "");
}
/** Call only after the destination durably accepts this item; mutates plain plan data. */
export function acknowledgeExport(plan: ExportPlan, index: number): void {
  if (!Number.isSafeInteger(index) || index < 0 || index >= plan.items.length)
    throw new RangeError("export item index");
  const id = plan.items[index].id;
  if (!plan.cursor.some(other => itemKey(other) === itemKey(id))) plan.cursor.push(structuredClone(id));
}

export type Event<P = PayloadId> =
  | { kind: "reset" }
  | { kind: "deliver"; hash: Bytes; author: Bytes; parents: Bytes[]; payload: P; active: boolean }
  | { kind: "activate" | "retract"; hash: Bytes }
  | { kind: "payload"; hash: Bytes; payload: P };
export interface Batch { token: bigint; rebuild: boolean; events: Event[] }
export interface Rejection { hash: Bytes; reason?: string; cascade?: boolean }
export interface Applied { rejected?: Rejection[]; wanted?: Bytes[] }
export interface CheckpointInfo {
  follow: Follow; heads: Bytes[]; partial: boolean; rejected: { hash: Bytes; cascade: boolean }[];
}
export const inspectCheckpoint = (bytes: Bytes): CheckpointInfo => inspect(bytes);
export interface MaterializationOptions {
  follow?: Follow; supportsVisibilityChanges?: boolean; checkpoint?: Bytes;
}
/**
 * Owns progress, not application state. Apply all events (including inactive
 * deliveries), then complete the unchanged batch. Finish every returned
 * follow-up before atomically saving state with checkpoint.
 * Cancellation/failure requires restoring state and checkpoint together.
 */
export class Materialization {
  readonly #r: Resource<MaterializationEngine>;
  constructor(options: MaterializationOptions = {}) {
    this.#r = resource(new MaterializationEngine(options.follow ?? "working",
      options.supportsVisibilityChanges ?? false, options.checkpoint));
  }
  dispose() { this.#r.dispose(); }
  get checkpoint(): Bytes { return this.#r.get().checkpoint(); }
  get heads(): Bytes[] { return this.#r.get().heads(); }
  get follow(): Follow { return this.#r.get().follow(); }
  setFollow(follow: Follow) { this.#r.get().setFollow(follow); }
  isCurrent(ledger: Ledger): boolean { return this.#r.get().isCurrent(native(ledger)); }
  plan(ledger: Ledger, budget = 1024): Batch | undefined {
    if (!Number.isSafeInteger(budget) || budget <= 0 || budget > 0xffffffff)
      throw new RangeError("budget must be a positive u32");
    return this.#r.get().plan(native(ledger), budget);
  }
  complete(ledger: Ledger, batch: Batch, applied: Applied = {}): Batch | undefined {
    return this.#r.get().complete(native(ledger), batch, applied);
  }
  markLocal(ledger: Ledger, draft: Draft): void { this.#r.get().markLocal(native(ledger), draft); }
}

/** An async decoder for caller-owned storage; output must not exceed capacity. */
export interface Decoder {
  step(input: Bytes, eof: boolean, capacity: number): MaybePromise<DecodeStep>;
}
export interface Host {
  read(offset: number, max: number): MaybePromise<Bytes>;
  decoder(meta: BundleMeta): MaybePromise<Decoder>;
}
export interface DriveOptions { signal?: AbortSignal }
const turn = () => new Promise<void>(resolve => setTimeout(resolve, 0));

function hostFailed(op: Admission | DecodeItem, ticket: bigint, cause: unknown): never {
  if (op.request()) {
    try { op.reply(ticket, { kind: "failed", message: String(cause) }); }
    catch { /* reply may report the terminal core error immediately */ }
  }
  const failed = op.next();
  if (failed.type === "failed") { failed.error.cause = cause; throw failed.error; }
  throw cause;
}

/** Verify only, without a ledger borrow. Persist/stage bytes and IDs before admit. */
export async function verify(admission: Admission, host: Host, options: DriveOptions = {}): Promise<void> {
  let decoder: Decoder | undefined;
  for (;;) {
    options.signal?.throwIfAborted();
    const next = admission.next();
    if (next.type === "needHistory") return;
    if (next.type === "continue") { await turn(); continue; }
    if (next.type === "failed") throw next.error;
    if (next.type !== "request") throw new Error("unexpected admission state: " + next.type);
    const { ticket, request } = next;
    try {
      if (request.kind === "read") {
        const bytes = await host.read(request.offset, request.max);
        admission.reply(ticket, { kind: "read", bytes, eof: bytes.length === 0 });
      } else if (request.kind === "decode") {
        decoder ??= await host.decoder(admission.meta()!);
        admission.reply(ticket, { kind: "decoded", ...await decoder.step(request.input, request.eof, request.capacity) });
      } else throw new Error("verification never requests bind");
    } catch (cause) {
      hostFailed(admission, ticket, cause);
    }
  }
}

/**
 * Decode with bounded requests and no ledger borrow across awaits. Chunks are
 * provisional until decoding succeeds: stage visitor effects until then.
 */
export async function driveDecode(
  decode: DecodeItem, host: Host,
  visit: (member: number, bytes: Bytes) => MaybePromise<void>,
  options: DriveOptions = {},
): Promise<void> {
  let decoder: Decoder | undefined;
  for (;;) {
    options.signal?.throwIfAborted();
    const next = decode.next();
    if (next.type === "done") return;
    if (next.type === "failed") throw next.error;
    if (next.type === "chunk") { await visit(next.member, next.bytes); continue; }
    if (next.type !== "request") throw new Error("unexpected decode state: " + next.type);
    const { ticket, request } = next;
    try {
      if (request.kind === "read") {
        const bytes = await host.read(request.offset, request.max);
        decode.reply(ticket, { kind: "read", bytes, eof: bytes.length === 0 });
      } else if (request.kind === "decode") {
        decoder ??= await host.decoder(decode.meta()!);
        decode.reply(ticket, { kind: "decoded", ...await decoder.step(request.input, request.eof, request.capacity) });
      } else throw new Error("decoding never requests bind");
    } catch (cause) { hostFailed(decode, ticket, cause); }
  }
}

/** Pure synchronous in-memory codec, distinct from caller-owned async I/O. */
export interface Codec {
  format: string;
  decoder(meta: BundleMeta): { step(input: Bytes, eof: boolean, capacity: number): DecodeStep };
  encode(meta: BundleMeta, payloads: Bytes[]): Bytes;
}
function concat(parts: Bytes[]): Bytes {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) { out.set(p, offset); offset += p.length; }
  return out;
}

/** In-memory convenience: synchronous local commits, receive, payloads and export. */
export class Document {
  readonly ledger: Ledger;
  readonly #codec: Codec;
  readonly #payloads = new Map<PayloadId, Bytes>();
  readonly #staging = new Map<string, { payloads: Bytes[]; references: number }>();
  readonly #pending = new Map<PayloadId, Bytes>();
  #next = 0n;
  constructor(codec: Codec, options: Omit<LedgerOptions, "format">) {
    this.#codec = codec;
    this.ledger = new Ledger({ ...options, format: codec.format });
  }
  dispose() { this.ledger.dispose(); this.#payloads.clear(); this.#staging.clear(); this.#pending.clear(); }
  get stagedItems(): number { return this.#staging.size; }
  #allocate(): PayloadId {
    if (this.#next < 0n || this.#next >= 2n ** 64n) throw new RangeError("payload ID space exhausted");
    return this.#next++;
  }
  commit(payload: Bytes, parents?: Bytes[]): Draft {
    const builder = this.ledger.beginCommit(parents);
    try {
      builder.update(payload);
      const id = this.#allocate();
      const draft = this.ledger.commit(builder, id);
      if (this.ledger.lookup(draft.hash)!.payload === id) this.#payloads.set(id, payload.slice());
      return draft;
    } finally { builder.dispose(); }
  }
  #bind: Bind = (id, members) => {
    const payloads = this.#staging.get(itemKey(id))?.payloads;
    if (!payloads || payloads.length !== members.length) throw new Error("missing staged payloads");
    return payloads.map(bytes => {
      const id = this.#allocate(); this.#pending.set(id, bytes); return id;
    });
  };
  #finish(receipts: Receipt[], failed: Failure[]) {
    for (const receipt of receipts) {
      for (const member of receipt.members) {
        const bytes = this.#pending.get(member.payload);
        if (bytes) this.#payloads.set(member.payload, bytes);
      }
      this.#release(itemKey(receipt.id));
    }
    for (const failure of failed) this.#release(itemKey(failure.id));
    this.#pending.clear();
  }
  #release(key: string) {
    const staged = this.#staging.get(key);
    if (staged && --staged.references === 0) this.#staging.delete(key);
  }
  attachSignature(draft: Draft, signature: Bytes): Signed {
    try {
      const result = this.ledger.attachSignature(draft, signature, this.#bind);
      this.#finish(result.released, result.failed);
      return result;
    } finally { this.#pending.clear(); }
  }
  receive(bytes: Bytes): Received {
    const admission = this.ledger.beginReceive();
    const parts: Bytes[][] = [[]];
    let decoder: ReturnType<Codec["decoder"]> | undefined;
    let key: string | undefined;
    let inserted = false;
    try {
      for (;;) {
        const next = admission.next();
        if (next.type === "continue") continue;
        if (next.type === "failed") throw next.error;
        if (next.type === "needHistory") break;
        if (next.type !== "request") throw new Error("unexpected verification state");
        const { ticket, request } = next;
        if (request.kind === "read") {
          const input = bytes.subarray(request.offset, request.offset + request.max);
          admission.reply(ticket, { kind: "read", bytes: input, eof: input.length === 0 });
        } else if (request.kind === "decode") {
          decoder ??= this.#codec.decoder(admission.meta()!);
          const step = decoder.step(request.input, request.eof, request.capacity);
          admission.reply(ticket, { kind: "decoded", ...step });
          parts[parts.length - 1].push(step.output.slice());
          if (step.status === "endMember") parts.push([]);
        } else throw new Error("verification never binds");
      }
      const meta = admission.meta()!;
      key = itemKey(meta.id);
      const staged = this.#staging.get(key);
      if (staged) staged.references++;
      else this.#staging.set(key, {
        payloads: parts.slice(0, meta.members.length).map(concat), references: 1,
      });
      inserted = true;
      const result = this.ledger.admit(admission, this.#bind);
      if (result.kind === "admitted") this.#finish(result.items, result.failed);
      else if (result.kind === "evicted") this.#release(itemKey(result.evicted));
      return result;
    } catch (error) {
      if (key && inserted) this.#release(key);
      throw error;
    } finally { admission.dispose(); this.#pending.clear(); }
  }
  payloadById(id: PayloadId): Bytes {
    const bytes = this.#payloads.get(id);
    if (!bytes) throw new Error("unknown payload ID");
    return bytes.slice();
  }
  payload(hash: Bytes): Bytes {
    const info = this.ledger.lookup(hash);
    if (!info) {
      this.ledger.checkPayload(hash).dispose(); // native UnknownCommit
      throw new Error("unknown commit");
    }
    return this.payloadById(info.payload);
  }
  export(cursor: ExportCursor = []): ExportPlan { return this.ledger.export(cursor); }
  /** Encode a captured description; no live ledger borrow or plan handle needed. */
  exportItem(item: ExportedItem): Bytes {
    return concat([item.header, this.#codec.encode(item.meta,
      item.members.map(m => this.payloadById(m.payload)))]);
  }
}
