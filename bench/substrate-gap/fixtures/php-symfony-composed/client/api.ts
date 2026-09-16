// TS client for the Symfony controller in ../server. The paths here are the
// COMPOSED ones (class prefix + action template) — they only pair with the
// graph once the PHP parser composes a class-level #[Route] onto its actions.
export async function show(id: string): Promise<unknown> {
  const res = await fetch(`/api/v1/users/${id}`);
  return res.json();
}

export async function create(body: unknown): Promise<void> {
  await fetch("/api/v1/users", { method: "POST", body: JSON.stringify(body) });
}
