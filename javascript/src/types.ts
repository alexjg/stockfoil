import { RawGraph } from "#wasm-bodge/bindings";

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

export interface Commit {
  readonly hash: CommitHash;
  readonly author: AuthorId;
  readonly parents: readonly CommitHash[];
  readonly payloadHash: string;
  readonly payload: Uint8Array;
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

export type TransportItem =
  | {
      readonly kind: "commit";
      readonly commit: Commit;
      readonly signatures: readonly SignatureEvidence[];
    }
  | {
      readonly kind: "fragment";
      readonly head: CommitHash;
      readonly boundary: readonly CommitHash[];
      readonly commits: readonly Commit[];
      readonly signatures: readonly SignatureEvidence[];
      /**
       * Tips of history this fragment does not authenticate itself; other
       * items must. Usually empty. Optional when sending.
       */
      readonly borrowed?: readonly CommitHash[];
    };

export interface MaterializationChange {
  readonly commit: Commit;
  readonly newlyDelivered: boolean;
  readonly visibility: "before" | "after" | "both" | "neither";
}

export type ErrorCode =
  | "InvalidInput"
  | "PayloadHashMismatch"
  | "CommitHashMismatch"
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
const shared = RawGraph as unknown as { [key: symbol]: typeof StockfoilError | undefined };
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
