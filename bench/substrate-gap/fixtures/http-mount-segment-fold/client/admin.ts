// An admin console whose base URL is opaque to the build: an env read the repo
// constant table never binds, so every path below normalises to `/{}/...` and
// reaches the HTTP resolver's base-fold tier (Kina's `${ADMIN_BASE}` shape).
const ADMIN = process.env.ADMIN_BASE;

// Served behind the server's `/admin` mount: the folded `/users/{}/status`
// is the route's tail, behind one literal mount segment.
export async function setStatus(id: string, status: string): Promise<void> {
  await fetch(`${ADMIN}/users/${id}/status`, { method: "POST", body: status });
}

// Served behind two literal mount segments (`/back/office`).
export async function auditLog(): Promise<unknown> {
  const res = await fetch(`${ADMIN}/audit/log`);
  return res.json();
}

// Ambiguous: the server serves `/reports` behind `/admin` AND behind `/ops`.
// Which one the base names is unknowable, so it pairs with neither.
export async function reports(): Promise<unknown> {
  const res = await fetch(`${ADMIN}/reports`);
  return res.json();
}

// The folded path is itself a route (`GET /health`): the base fold's own
// tiers win, and the mounted `/admin/health` is never reached.
export async function health(): Promise<unknown> {
  const res = await fetch(`${ADMIN}/health`);
  return res.json();
}
