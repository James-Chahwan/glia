// HTTP_CALLS probe (TypeScript client -> Express server route, separate dirs).
export async function getUser(id: string): Promise<unknown> {
  const res = await fetch(`/users/${id}`);
  return res.json();
}

export async function createUser(body: unknown): Promise<void> {
  await fetch("/users", { method: "POST", body: JSON.stringify(body) });
}
