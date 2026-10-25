// One API surface for both shells: Tauri commands in the desktop app, HTTP
// against ccm-server in browser / headless mode.
import { invoke } from "@tauri-apps/api/core";

export interface Status {
  version: string;
  listen: string;
  accounts: number;
  upstream: string;
}

export const isTauri = "__TAURI_INTERNALS__" in window;

export async function getStatus(): Promise<Status> {
  if (isTauri) {
    return invoke<Status>("core_status");
  }
  const resp = await fetch("/healthz");
  if (!resp.ok) {
    throw new Error(`${resp.status} ${await resp.text()}`);
  }
  return resp.json();
}
