// Pairs with the COMPOSED FastAPI route `/api/v1/users/{id}`. Before prefix
// composition the server route is `/{id}`, so this fetch pairs with nothing.
export async function getUser(id: string): Promise<unknown> {
  const res = await fetch(`/api/v1/users/${id}`);
  return res.json();
}
