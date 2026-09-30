import { RawCodecGraph } from "#wasm-bodge/bindings";

export type CommitHash = string & { readonly __brand: "CommitHash" };
export type AuthorId = string & { readonly __brand: "AuthorId" };

export type WritePermission =
  | "unbounded"
  | "denied"
  | { readonly through: readonly CommitHash[] };

export interface ViewPoint {
  readonly heads: readonly CommitHash[];
  readonly writePermissions: ReadonlyMap<AuthorId, WritePermission>;
}

export interface Commit<P = Uint8Array> {
  readonly hash: CommitHash;
  readonly author: AuthorId;
  readonly parents: readonly CommitHash[];
  readonly payloadHash: string;
  readonly payload: P;
}

export interface SignatureEvidence {
  readonly author: AuthorId;
  readonly commit: CommitHash;
  readonly signature: Uint8Array;
}

export type ItemId =
  | { readonly kind: "commit"; readonly hash: CommitHash }
  | {
      readonly kind: "fragment";
      readonly head: CommitHash;
      readonly boundary: readonly CommitHash[];
    };

export interface MaterializationChange<P = Uint8Array> {
  readonly commit: Commit<P>;
  readonly newlyDelivered: boolean;
  readonly visibility: "before" | "after" | "both" | "neither";
}

export type ErrorCode =
  | "CodecError"
  | "InvalidEncoding"
  | "WrongCodec"
  | "NonCanonicalPayload"
  | "LimitExceeded"
  | "ReentrantCall"
  | "InvalidInput"
  | "PayloadHashMismatch"
  | "InvalidSignature"
  | "InvalidPublicKey"
  | "MissingSignature"
  | "InvalidItem"
  | "InvalidFragment"
  | "NotPendingLocalCommit"
  | "MissingCommit"
  | "Disposed"
  | "InternalError";

class StockfoilError extends Error {
  constructor(readonly code: ErrorCode, message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "StockfoilError";
  }
}

// Root and slim wrappers share the raw constructor, but are bundled separately.
// Keep error identity shared too, so libraries can use instanceof across them.
const errorKey = Symbol.for("stockfoil.StockfoilError");
const shared = RawCodecGraph as unknown as { [key: symbol]: typeof StockfoilError | undefined };
const SharedStockfoilError = shared[errorKey] ??= StockfoilError;
type SharedStockfoilError = StockfoilError;
export { SharedStockfoilError as StockfoilError };

export interface IngestRejection {
  readonly item: ItemId;
  readonly error: StockfoilError;
}

export interface IngestReport {
  readonly admitted: readonly ItemId[];
  readonly rejected: readonly IngestRejection[];
  readonly pending: number;
}

/** Inbound per-item budgets only; not local commit/export limits or pending-queue quotas. */
export interface DecodeLimits {
  readonly maxItemBytes: number;
  readonly maxMembers: number;
  readonly maxParents: number;
  readonly maxSignatures: number;
  readonly maxPayloadBytes: number;
}

export type ParentRef =
  | { readonly kind: "member"; readonly index: number }
  | { readonly kind: "external"; readonly hash: CommitHash };

export interface FragmentShape {
  readonly members: readonly {
    readonly author: AuthorId;
    readonly parents: readonly ParentRef[];
  }[];
}

/** All callbacks are synchronous. Values must support structuredClone. */
export interface PayloadCodec<P> {
  readonly formatId: string;
  encodeCommit(payload: P): Uint8Array;
  decodeCommit(bytes: Uint8Array, limits: DecodeLimits): P;
  encodeFragment(shape: FragmentShape, payloads: readonly P[]): Uint8Array;
  decodeFragment(shape: FragmentShape, bytes: Uint8Array, limits: DecodeLimits): P[];
}

export type TransportMetadata =
  | { readonly kind: "commit"; readonly hash: CommitHash; readonly parents: readonly CommitHash[] }
  | {
      readonly kind: "fragment";
      readonly head: CommitHash;
      readonly boundary: readonly CommitHash[];
      readonly checkpoints: readonly Uint8Array[];
    };

export interface EncodedItem {
  readonly metadata: TransportMetadata;
  readonly bytes: Uint8Array;
}
