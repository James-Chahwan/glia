// Client half: the fetch template must pair with the COMPOSED rails route
// (`/api/v1/users/:id`), not the bare `/users/:id` the walker emitted before
// namespace/scope composition landed.
export async function show(id: string): Promise<unknown> {
  const res = await fetch(`/api/v1/users/${id}`);
  return res.json();
}
