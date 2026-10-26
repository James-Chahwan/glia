// The app's one fetch wrapper: every request goes through `api(path, opts)`,
// which prefixes the API base. No extractor knows this function.
const API_BASE = "/api";

export async function api(path: string, opts: RequestInit = {}) {
  const res = await fetch(`${API_BASE}${path}`, { credentials: "include", ...opts });
  if (!res.ok) {
    throw new Error(res.statusText);
  }
  return res.json();
}
