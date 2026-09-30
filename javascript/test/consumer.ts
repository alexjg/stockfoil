// Compiled again from an installed tarball, without this project's path aliases.
import {
  CommitGraph, MaterializationCursor, ExportCursor, StockfoilError,
  type PayloadCodec, type EncodedItem, type FragmentShape, type DecodeLimits,
  authorId, commitHash, describeCommit,
  type CommitHash, type WritePermission,
} from "stockfoil";
import init, {
  initSync, CommitGraph as SlimGraph, MaterializationCursor as SlimCursor,
} from "stockfoil/slim";

const author = authorId(new Uint8Array(32));
const byteCodec: PayloadCodec<Uint8Array> = {
  formatId: "test/bytes/v1",
  encodeCommit: value => value, decodeCommit: value => value,
  encodeFragment: (_shape, values) => new TextEncoder().encode(JSON.stringify(values.map(v => [...v]))),
  decodeFragment: (_shape, bytes) => (JSON.parse(new TextDecoder().decode(bytes)) as number[][]).map(v => Uint8Array.from(v)),
};
const graph = new CommitGraph(byteCodec);
const permissions = new Map([[author, "unbounded" as WritePermission]]);
graph.setWritePermissions(permissions);
const draft = graph.commit(author, new Uint8Array());
const hash: CommitHash = commitHash(draft.signingBytes);
const commit = describeCommit(author, [hash, hash], new Uint8Array());
const item = { kind: "commit", commit, signatures: [] };
// @ts-expect-error structured transport is not an ingestion API
graph.ingest(item);
graph.addSignature({ author, commit: hash, signature: new Uint8Array(64) });
graph.changes(new SlimCursor(), graph.signedViewPoint());
new SlimGraph(byteCodec).changes(new MaterializationCursor(), graph.viewPoint());
const published = graph.exportChanges(new ExportCursor());
const exports: readonly EncodedItem[] = published.added;
for (const item of exports) {
  graph.ingest(item.bytes);
  if (item.metadata.kind === "fragment") {
    const prefixes: readonly Uint8Array[] = item.metadata.checkpoints;
    prefixes.forEach(prefix => prefix.byteLength);
  } else {
    // @ts-expect-error standalone commits have no checkpoint metadata
    item.metadata.checkpoints;
  }
}
graph.exportChanges(published.cursor.clone());
graph.isAncestor(hash, draft.hash);
graph.hasCommit(hash);
graph.missingDependencies();
graph.pendingItems();
const error: StockfoilError = new StockfoilError("InvalidInput", "bad input");
if (error instanceof StockfoilError) error.code;
initSync(new Uint8Array());
void init(Promise.resolve(new Response()));

// @ts-expect-error cursors have distinct nominal types
graph.changes(new ExportCursor());
// @ts-expect-error identifiers must be constructed/validated, not arbitrary strings
graph.commit("not an author", new Uint8Array());
// @ts-expect-error inputs are bytes, not arbitrary application values
graph.commit(author, { key: "title", value: "hello" });
// @ts-expect-error raw Wasm pointers are not public
graph.__wbg_ptr;
// @ts-expect-error no second graph or unchecked insertion escape hatch
graph.raw();
graph.dispose();

interface Assignment { key: string; value: string }
const codec: PayloadCodec<Assignment> = {
  formatId: "test/assignment/v1",
  encodeCommit(payload) { return new TextEncoder().encode(JSON.stringify(payload)); },
  decodeCommit(bytes, limits) {
    const max: number = limits.maxPayloadBytes;
    void max;
    return JSON.parse(new TextDecoder().decode(bytes)) as Assignment;
  },
  encodeFragment(shape, payloads) {
    const parent = shape.members[0]?.parents[0];
    if (parent?.kind === "member") { const index: number = parent.index; void index; }
    else if (parent) { const hash: CommitHash = parent.hash; void hash; }
    // @ts-expect-error codec batches are readonly
    payloads.push({ key: "bad", value: "bad" });
    return new TextEncoder().encode(JSON.stringify(payloads));
  },
  decodeFragment(_shape: FragmentShape, bytes: Uint8Array, _limits: DecodeLimits) {
    return JSON.parse(new TextDecoder().decode(bytes)) as Assignment[];
  },
};
const typed = new CommitGraph(codec, { maxMembers: 100 });
typed.setDecodeLimits({ maxPayloadBytes: 16 * 1024 * 1024 });
typed.setDecodeLimits({});
// @ts-expect-error budgets must be numeric
typed.setDecodeLimits({ maxItemBytes: 'large' });
// @ts-expect-error only known decode budgets are accepted
typed.setDecodeLimits({ maxQueueItems: 10 });
const typedDraft = typed.commit(author, { key: "title", value: "hello" });
const prepared: Assignment = typed.describeCommit(author, { key: "x", value: "y" }).payload;
void prepared;
const typedBatch = typed.changes(new SlimCursor());
const value: Assignment | undefined = typedBatch.changes[0]?.commit.payload;
void value;
// @ts-expect-error transfer goes through exportChanges, not item-by-ID export
typed.export({ kind: "commit", hash: typedDraft.hash });
const exportBatch = typed.exportChanges(new ExportCursor());
const encodedItems: readonly EncodedItem[] = exportBatch.added;
for (const item of encodedItems) {
  typed.ingest(item.bytes);
  if (item.metadata.kind === "fragment") {
    const checkpoints: readonly Uint8Array[] = item.metadata.checkpoints;
    void checkpoints;
  } else {
    const parents: readonly CommitHash[] = item.metadata.parents;
    void parents;
    // @ts-expect-error standalone metadata has no checkpoints
    item.metadata.checkpoints;
  }
}
new SlimGraph(codec).changes(typedBatch.cursor);
// @ts-expect-error the primary graph requires a codec
new CommitGraph();
// @ts-expect-error typed payload does not accept arbitrary bytes
typed.commit(author, new Uint8Array());
// @ts-expect-error ingestion takes a native encoded item, not structured transport
typed.ingest(item);
// @ts-expect-error synchronous codec methods cannot return promises
const asyncCodec: PayloadCodec<Assignment> = { ...codec, encodeCommit: async () => new Uint8Array() };
void asyncCodec;
// @ts-expect-error required fragment encoder has no default
new CommitGraph({ formatId: "x", encodeCommit: codec.encodeCommit, decodeCommit: codec.decodeCommit, decodeFragment: codec.decodeFragment });
typed.dispose();
