import { api } from "./http";

// One level of forwarding: the verb helpers hand their own `path` to `api`.
export function getJson(path: string) {
  return api(path, { method: "GET" });
}

export function sendJson(method: string, path: string, body: unknown) {
  return api(path, { method, body: JSON.stringify(body) });
}

// Two levels: a versioned resource reader forwards a prefixed path to getJson.
export function readResource(path: string) {
  return getJson(`/v1${path}`);
}
