// HTTP_CALLS probe for a composed ASP.NET route (TypeScript client, separate dir).
export async function getUser(id: number): Promise<unknown> {
  const res = await fetch(`/api/users/${id}`);
  return res.json();
}
