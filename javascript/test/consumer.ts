// Compiled again from an installed tarball, without this project's path aliases.
import {
  CommitGraph, MaterializationCursor, ExportCursor, StockfoilError,
  authorId, commitHash, describeCommit,
  type CommitHash, type WritePermission, type TransportItem,
} from "stockfoil";
import init, {
  initSync, CommitGraph as SlimGraph, MaterializationCursor as SlimCursor,
} from "stockfoil/slim";

const author = authorId(new Uint8Array(32));
const graph = new CommitGraph();
const permissions = new Map([[author, "unbounded" as WritePermission]]);
graph.setWritePermissions(permissions);
const draft = graph.commit(author, new Uint8Array());
const hash: CommitHash = commitHash(draft.signingBytes);
const commit = describeCommit(author, [hash, hash], new Uint8Array());
const item: TransportItem = { kind: "commit", commit, signatures: [] };
graph.ingest(item);
graph.addSignature({ author, commit: hash, signature: new Uint8Array(64) });
graph.changes(new SlimCursor(), graph.signedViewPoint());
new SlimGraph().changes(new MaterializationCursor(), graph.viewPoint());
const published = graph.exportChanges(new ExportCursor());
const exports: readonly TransportItem[] = published.added;
for (const item of exports) {
  graph.ingest(item); // Checkpoint hints do not establish authenticity.
  if (item.kind === "fragment") {
    const prefixes: readonly Uint8Array[] = item.checkpoints;
    prefixes.forEach(prefix => prefix.byteLength);
  } else {
    // @ts-expect-error standalone commits have no checkpoint metadata
    item.checkpoints;
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
graph.dispose();
