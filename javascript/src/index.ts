import {
  RawCodecGraph, RawMaterializationCursor, RawExportCursor,
  authorId as rawAuthorId, commitHash as rawCommitHash,
  describeCommit as rawDescribeCommit,
} from "#wasm-bodge/bindings";
import {
  StockfoilError, type ErrorCode, type AuthorId, type CommitHash, type Commit,
  type WritePermission, type ViewPoint, type SignatureEvidence,
  type ItemId, type IngestReport, type MaterializationChange,
  type PayloadCodec, type DecodeLimits, type EncodedItem,
} from "./types.js";
export * from "./types.js";
import { codecBridge, limits } from "./codec.js";

function call<T>(fn: () => T): T {
  try { return fn(); }
  catch (error) {
    if (error instanceof StockfoilError) throw error;
    if (error instanceof Error && error.name === "StockfoilError" && "code" in error) {
      throw new StockfoilError(error.code as ErrorCode, error.message, {
        cause: "cause" in error ? error.cause : error,
      });
    }
    throw error;
  }
}

function bytes(value: Uint8Array): Uint8Array {
  if (!(value instanceof Uint8Array)) {
    throw new StockfoilError("InvalidInput", "expected Uint8Array");
  }
  return value;
}

function identifier(value: string): string {
  if (typeof value !== "string") {
    throw new StockfoilError("InvalidInput", "expected a hexadecimal identifier");
  }
  // Rust validates canonical spelling and length before using it.
  return value;
}

/** Convert 32 public-key bytes to a canonical author identifier. */
export function authorId(value: Uint8Array): AuthorId {
  return call(() => rawAuthorId(bytes(value))) as AuthorId;
}

/** Convert a raw 32-byte hash (not arbitrary contents) to an identifier. */
export function commitHash(value: Uint8Array): CommitHash {
  return call(() => rawCommitHash(bytes(value))) as CommitHash;
}

/** Derive hashes for decoded contents. This does not authenticate the commit. */
export function describeCommit(
  author: AuthorId, parents: readonly CommitHash[], payload: Uint8Array,
): Commit {
  return call(() => rawDescribeCommit(identifier(author), parents, bytes(payload))) as Commit;
}

// Raw handles have finalizers. Keep them alive while public cursors are reachable.
// wasm-bodge bundles the TS wrapper separately for root/slim, but shares the raw
// constructors. Anchor these maps there so a library's slim cursors work with an
// application's root graph, without sharing pointers between Wasm instances.
function cursorMap<C extends object, R>(constructor: object, name: string): WeakMap<C, R> {
  const key = Symbol.for(`stockfoil.${name}`);
  const shared = constructor as { [key: symbol]: WeakMap<C, R> | undefined };
  return shared[key] ??= new WeakMap<C, R>();
}
const materializationCursors = cursorMap<MaterializationCursor, RawMaterializationCursor>(
  RawMaterializationCursor, "materializationCursors",
);
const exportCursors = cursorMap<ExportCursor, RawExportCursor>(RawExportCursor, "exportCursors");

function handle<C extends object, R>(handles: WeakMap<C, R>, cursor: C): R {
  const raw = handles.get(cursor);
  if (!raw) throw new StockfoilError("InvalidInput", "expected a Stockfoil cursor");
  return raw;
}

function wrapMaterializationCursor(raw: RawMaterializationCursor): MaterializationCursor {
  const cursor = Object.create(MaterializationCursor.prototype) as MaterializationCursor;
  materializationCursors.set(cursor, raw);
  return cursor;
}

function wrapExportCursor(raw: RawExportCursor): ExportCursor {
  const cursor = Object.create(ExportCursor.prototype) as ExportCursor;
  exportCursors.set(cursor, raw);
  return cursor;
}

/** Delivery and participation already processed by a materializer. */
export class MaterializationCursor {
  // Nominal types prevent exchanging materialization and export cursors in TS.
  declare private readonly materializationCursor: void;
  constructor() { materializationCursors.set(this, new RawMaterializationCursor()); }
  clone(): MaterializationCursor {
    return call(() => wrapMaterializationCursor(handle(materializationCursors, this).clone()));
  }
}

/** Certified inventory already stored at one publication destination. */
export class ExportCursor {
  declare private readonly exportCursor: void;
  constructor() { exportCursors.set(this, new RawExportCursor()); }
  clone(): ExportCursor {
    return call(() => wrapExportCursor(handle(exportCursors, this).clone()));
  }
}

export interface MaterializationBatch<P = Uint8Array> {
  readonly changes: readonly MaterializationChange<P>[];
  readonly cursor: MaterializationCursor;
}

interface RawReport {
  admitted: ItemId[];
  rejected: { item: ItemId; error: { code: ErrorCode; message: string } }[];
  pending: number;
}

function report(raw: RawReport): IngestReport {
  return {
    admitted: raw.admitted,
    rejected: raw.rejected.map(({ item, error }) => ({
      item, error: new StockfoilError(error.code, error.message),
    })),
    pending: raw.pending,
  };
}

/** Complete native envelopes and the captured publication inventory cursor. */
export interface ExportBatch {
  readonly added: readonly EncodedItem[];
  readonly retired: readonly ItemId[];
  readonly cursor: ExportCursor;
}

/** A typed graph using the native Stockfoil envelope and an application codec. */
export class CommitGraph<P> {
  #raw: RawCodecGraph | undefined;
  #busy = false;
  #maxItemBytes: number;

  constructor(codec: PayloadCodec<P>, options: Partial<DecodeLimits> = {}) {
    const bridge = codecBridge(codec);
    const budget = limits(options);
    this.#maxItemBytes = budget.maxItemBytes;
    this.#raw = call(() => new RawCodecGraph(bridge, bridge.formatId, budget));
  }

  #run<T>(operation: (graph: RawCodecGraph) => T): T {
    if (this.#busy) throw new StockfoilError("ReentrantCall", "graph operation in progress");
    if (!this.#raw) throw new StockfoilError("Disposed", "graph has been disposed");
    this.#busy = true;
    try { return call(() => operation(this.#raw!)); }
    finally { this.#busy = false; }
  }

  commit(author: AuthorId, payload: P): {
    hash: CommitHash; signingBytes: Uint8Array; report: IngestReport;
  } {
    const draft = this.#run(g => g.commit(identifier(author), payload)) as {
      hash: CommitHash; signingBytes: Uint8Array; report: RawReport;
    };
    return { ...draft, report: report(draft.report) };
  }

  /** Prepare metadata using current working heads, without inserting a draft. */
  describeCommit(author: AuthorId, payload: P): Commit<P> {
    return this.#run(g => g.describeCommit(identifier(author), payload)) as Commit<P>;
  }

  addSignature(evidence: SignatureEvidence): IngestReport {
    return report(this.#run(g => g.addSignature(evidence)) as RawReport);
  }

  /** Replace inbound budgets: omitted options reset to constructor defaults. */
  setDecodeLimits(options: Partial<DecodeLimits>): void {
    this.#run(g => {
      const budget = limits(options);
      g.setDecodeLimits(budget);
      this.#maxItemBytes = budget.maxItemBytes;
    });
  }

  /** Decode and authenticate one native envelope before admission or queuing. */
  ingest(item: Uint8Array): IngestReport {
    return report(this.#run(g => {
      const input = bytes(item);
      // Avoid even copying an oversized envelope into Wasm linear memory.
      if (input.byteLength > this.#maxItemBytes) {
        throw new StockfoilError("LimitExceeded", "encoded item bytes exceed maxItemBytes");
      }
      return g.ingest(input);
    }) as RawReport);
  }

  exportChanges(since: ExportCursor): ExportBatch {
    const batch = this.#run(g => g.exportChanges(handle(exportCursors, since))) as {
      added: EncodedItem[]; retired: ItemId[]; cursor: RawExportCursor;
    };
    return { added: batch.added, retired: batch.retired, cursor: wrapExportCursor(batch.cursor) };
  }

  changes(since: MaterializationCursor, view?: ViewPoint): MaterializationBatch<P> {
    const batch = this.#run(g => g.changes(handle(materializationCursors, since), view)) as {
      changes: MaterializationChange<P>[]; cursor: RawMaterializationCursor;
    };
    return { changes: batch.changes, cursor: wrapMaterializationCursor(batch.cursor) };
  }

  viewPoint(): ViewPoint { return this.#run(g => g.viewPoint()) as ViewPoint; }
  signedViewPoint(): ViewPoint { return this.#run(g => g.signedViewPoint()) as ViewPoint; }
  setWritePermissions(permissions: ReadonlyMap<AuthorId, WritePermission>): void {
    this.#run(g => g.setWritePermissions(permissions));
  }
  hasCommit(hash: CommitHash): boolean { return this.#run(g => g.hasCommit(identifier(hash))); }
  isAncestor(ancestor: CommitHash, descendant: CommitHash): boolean {
    return this.#run(g => g.isAncestor(identifier(ancestor), identifier(descendant)));
  }
  pendingItems(): number { return this.#run(g => g.pendingItems()); }
  missingDependencies(): readonly CommitHash[] {
    return this.#run(g => g.missingDependencies()) as CommitHash[];
  }

  /** Idempotent, except that disposal during a codec callback is rejected. */
  dispose(): void {
    if (this.#busy) throw new StockfoilError("ReentrantCall", "graph operation in progress");
    if (this.#raw) { this.#raw.free(); this.#raw = undefined; }
  }
}
