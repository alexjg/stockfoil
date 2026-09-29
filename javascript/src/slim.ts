import initialize, { initSync as initializeSync } from "#wasm-bodge/bindings/slim";
export * from "./index.js";

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

/** Initialize manually, without exposing the generated raw Wasm exports. */
export default async function init(input: InitInput | Promise<InitInput>): Promise<void> {
  await initialize({ module_or_path: input });
}

export function initSync(module: BufferSource | WebAssembly.Module): void {
  initializeSync({ module });
}
