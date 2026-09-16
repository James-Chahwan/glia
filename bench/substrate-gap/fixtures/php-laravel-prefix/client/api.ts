// Pairs with the COMPOSED Laravel route `/api/v1/users/{id}`. Before prefix
// composition the server route is `/users/{id}`, so this fetch pairs with
// nothing.
export async function show(id: string): Promise<unknown> {
  const res = await fetch(`/api/v1/users/${id}`);
  return res.json();
}
