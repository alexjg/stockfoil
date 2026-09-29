import {
  Subduction, MemorySigner, MemoryStorage, PeerId, SedimentreeId, CommitId,
} from '@automerge/subduction';
import { ExportCursor } from 'stockfoil';
import { bytes, encode, decode } from './wire.mjs';

export async function bounded(promise, ms = 10_000) {
  let timer;
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('operation timed out')), ms); }),
    ]);
  } finally { clearTimeout(timer); }
}

export class Replica {
  published = new ExportCursor();
  constructor({ peerId, documentId }) {
    this.node = new Subduction({
      signer: new MemorySigner(), storage: new MemoryStorage(),
      defaultTimeoutMilliseconds: 5_000,
    });
    this.peer = new PeerId(bytes(peerId));
    this.id = SedimentreeId.fromBytes(bytes(documentId));
  }
  static async connect(config) {
    const replica = new Replica(config);
    try {
      // This is the published Subduction Wasm WebSocket client, not a proxy.
      const peer = await bounded(replica.node.connect(new URL(config.serverUrl), replica.peer));
      if (Buffer.from(peer.toBytes()).toString('hex') !== config.peerId) throw new Error('server identity mismatch');
      return replica;
    } catch (error) { await replica.close(); throw error; }
  }
  async publish(doc) {
    const batch = doc.graph.exportChanges(this.published);
    const id = hash => new CommitId(bytes(hash));
    for (const item of batch.added) {
      const blob = encode(item);
      if (item.kind === 'commit') {
        const parents = [...new Set(item.commit.parents)];
        if (parents.length > 255) throw new Error('too many transport parents');
        await bounded(this.node.addCommit(this.id, id(item.commit.hash), parents.map(id), blob));
      } else {
        const checkpoints = item.commits
          .filter(c => c.hash !== item.head && c.hash.startsWith('00')).map(c => id(c.hash));
        if (item.boundary.length > 255 || checkpoints.length > 65535) throw new Error('fragment transport limit exceeded');
        await bounded(this.node.addFragment(this.id, id(item.head), item.boundary.map(id), checkpoints, blob));
      }
    }
    // Retired cover entries need not be deleted. Advance only after all stores.
    this.published = batch.cursor;
    return batch.added;
  }
  async sync() {
    // Use Subduction's own deadline; racing a promise alone cannot cancel sync.
    const result = await bounded(this.node.syncWithPeer(this.peer, this.id, true, 5_000));
    try {
      if (!result.success || result.transportErrors.length) throw new Error('Subduction sync failed');
    } finally { result.free(); }
  }
  async blobs() { return bounded(this.node.getBlobs(this.id)); }
  async load(doc, reverse = false) {
    const blobs = await this.blobs();
    if (reverse) blobs.reverse();
    return doc.ingest(blobs.map(decode));
  }
  async close() {
    if (!this.node) return;
    try { await bounded(this.node.disconnectAll(), 2_000); }
    finally { this.node.free(); this.node = undefined; }
  }
}
