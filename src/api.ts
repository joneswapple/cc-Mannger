// One API surface for both shells. Every call is an `/admin/api/*` request:
// the desktop app routes it in-process through the `admin_request` Tauri
// command (no token needed), the browser sends it over HTTP with the admin
// token from the login screen.
import { invoke } from "@tauri-apps/api/core";

export const isTauri = "__TAURI_INTERNALS__" in window;

const TOKEN_KEY = "ccm_admin_token";

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

export function getToken(): string {
  try {
    return localStorage.getItem(TOKEN_KEY) ?? "";
  } catch {
    return "";
  }
}

export function setToken(token: string) {
  try {
    if (token) localStorage.setItem(TOKEN_KEY, token);
    else localStorage.removeItem(TOKEN_KEY);
  } catch {
    // Storage blocked (private mode): the token lives only in memory.
  }
  memoryToken = token;
}

let memoryToken = getToken();

function errorMessage(body: unknown, status: number): string {
  if (body && typeof body === "object" && "error" in body) {
    const e = (body as { error: unknown }).error;
    if (typeof e === "string") return e;
  }
  return typeof body === "string" && body ? body : `HTTP ${status}`;
}

export async function api<T = unknown>(method: "GET" | "POST", path: string, body?: unknown): Promise<T> {
  const full = `/admin/api${path}`;
  if (isTauri) {
    const r = await invoke<{ status: number; body: unknown }>("admin_request", {
      method,
      path: full,
      body: body ?? null,
    });
    if (r.status >= 400) throw new ApiError(r.status, errorMessage(r.body, r.status));
    return r.body as T;
  }
  const resp = await fetch(full, {
    method,
    headers: {
      Authorization: `Bearer ${memoryToken}`,
      ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const text = await resp.text();
  let parsed: unknown = null;
  if (text) {
    try {
      parsed = JSON.parse(text);
    } catch {
      parsed = text;
    }
  }
  if (!resp.ok) throw new ApiError(resp.status, errorMessage(parsed, resp.status));
  return parsed as T;
}

export const get = <T>(path: string) => api<T>("GET", path);
export const post = <T = unknown>(path: string, body?: unknown) => api<T>("POST", path, body ?? {});

/// Opens a URL in the system browser (the desktop webview can't).
export async function openExternal(url: string) {
  if (isTauri) {
    await invoke("open_url", { url });
  } else {
    window.open(url, "_blank", "noopener");
  }
}
