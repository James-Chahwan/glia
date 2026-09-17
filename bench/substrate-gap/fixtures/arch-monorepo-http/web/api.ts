// The `web` service: a browser client calling the `api` service over HTTP.
// Copied from tests/fixtures/arch_monorepo (bench/ is outside the cargo
// workspace, so it keeps its own copy rather than depending on tests/).
export async function listUsers(): Promise<unknown> {
  const res = await fetch("/users");
  return res.json();
}

export async function createUser(body: unknown): Promise<void> {
  await fetch("/users", { method: "POST", body: JSON.stringify(body) });
}
