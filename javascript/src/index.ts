import {
  RawGraph, RawMaterializationCursor, RawExportCursor,
  authorId as rawAuthorId, commitHash as rawCommitHash,
  describeCommit as rawDescribeCommit,
} from "#wasm-bodge/bindings";
import {
  StockfoilError, type ErrorCode, type AuthorId, type CommitHash, type Commit,
  type WritePermission, type ViewPoint, type SignatureEvidence,
  type TransportItem, type ItemId, type IngestReport, type MaterializationChange,
} from "./types.js";
export * from "./types.js";

function call<T>(fn: () => T): T {
  try { return fn(); }
  catch (error) {
    if (error instanceof StockfoilError) throw error;
    if (error instanceof Error && error.name === "StockfoilError" && "code" in error) {
      throw new StockfoilError(error.code as ErrorCode, error.message, { cause: error });
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

export interface MaterializationBatch {
  readonly changes: readonly MaterializationChange[];
  readonly cursor: MaterializationCursor;
}

export interface ExportBatch {
  readonly added: readonly TransportItem[];
  readonly retired: readonly ItemId[];
  readonly cursor: ExportCursor;
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

export class CommitGraph {
  #raw: RawGraph | undefined = new RawGraph();

  #graph(): RawGraph {
    if (!this.#raw) throw new StockfoilError("Disposed", "graph has been disposed");
    return this.#raw;
  }

  /** Add an unsigned local draft using the current working heads. */
  commit(author: AuthorId, payload: Uint8Array): {
    hash: CommitHash; signingBytes: Uint8Array; report: IngestReport;
  } {
    const draft = call(() => this.#graph().commit(identifier(author), bytes(payload))) as {
      hash: CommitHash; signingBytes: Uint8Array; report: RawReport;
    };
    return { ...draft, report: report(draft.report) };
  }

  addSignature(evidence: SignatureEvidence): IngestReport {
    return report(call(() => this.#graph().addSignature(evidence)) as RawReport);
  }

  /** Validate all contents and signatures, then admit or queue the received item. */
  ingest(item: TransportItem): IngestReport {
    return report(call(() => this.#graph().ingest(item)) as RawReport);
  }

  viewPoint(): ViewPoint { return call(() => this.#graph().viewPoint()) as ViewPoint; }
  signedViewPoint(): ViewPoint { return call(() => this.#graph().signedViewPoint()) as ViewPoint; }

  /** Replace permissions. Unlisted authors are denied. */
  setWritePermissions(permissions: ReadonlyMap<AuthorId, WritePermission>): void {
    call(() => this.#graph().setWritePermissions(permissions));
  }

  /** Owned changes in ancestors-before-descendants order; defaults to the working view. */
  changes(since: MaterializationCursor, view?: ViewPoint): MaterializationBatch {
    const batch = call(() => this.#graph().changes(handle(materializationCursors, since), view)) as {
      changes: MaterializationChange[]; cursor: RawMaterializationCursor;
    };
    return { changes: batch.changes, cursor: wrapMaterializationCursor(batch.cursor) };
  }

  /** Advance the returned cursor only after successful publication of the batch. */
  exportChanges(since: ExportCursor): ExportBatch {
    const batch = call(() => this.#graph().exportChanges(handle(exportCursors, since))) as {
      added: TransportItem[]; retired: ItemId[]; cursor: RawExportCursor;
    };
    return { added: batch.added, retired: batch.retired, cursor: wrapExportCursor(batch.cursor) };
  }

  /** Whether the commit belongs to admitted, authenticated history. */
  hasCommit(hash: CommitHash): boolean {
    return call(() => this.#graph().hasCommit(identifier(hash)));
  }

  isAncestor(ancestor: CommitHash, descendant: CommitHash): boolean {
    return call(() => this.#graph().isAncestor(identifier(ancestor), identifier(descendant)));
  }

  pendingItems(): number { return call(() => this.#graph().pendingItems()); }
  missingDependencies(): readonly CommitHash[] {
    return call(() => this.#graph().missingDependencies()) as CommitHash[];
  }

  /**
   * Distinct borrowed commits of queued fragments which no received item has
   * authenticated yet. Any item authenticating them unblocks those fragments.
   */
  missingAuthentication(): readonly CommitHash[] {
    return call(() => this.#graph().missingAuthentication()) as CommitHash[];
  }

  /** Release the graph's Wasm memory. Safe to call more than once. */
  dispose(): void {
    if (this.#raw) {
      this.#raw.free();
      this.#raw = undefined;
    }
  }
}
