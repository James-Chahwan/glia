// The `web` service: a browser client calling the `api` service over HTTP.
// Byte-identical in tests/fixtures/arch_monorepo and arch_nested/services —
// the only variable between those two fixtures is the directory depth.
export async function listUsers(): Promise<unknown> {
  const res = await fetch("/users");
  return res.json();
}

export async function createUser(body: unknown): Promise<void> {
  await fetch("/users", { method: "POST", body: JSON.stringify(body) });
}
