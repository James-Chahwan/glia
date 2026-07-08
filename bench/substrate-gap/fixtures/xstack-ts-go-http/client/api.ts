// Positive control for HTTP_CALLS on a SUPPORTED stack (TS client -> Go route).
// If this pairs but dart-http-dio does not, the gap is isolated to Dart
// endpoint extraction, not the HttpStackResolver.
export async function fetchUser(id: string): Promise<unknown> {
  const res = await fetch(`/users/${id}`);
  return res.json();
}

export async function createUser(body: unknown): Promise<void> {
  await fetch("/users", { method: "POST", body: JSON.stringify(body) });
}
