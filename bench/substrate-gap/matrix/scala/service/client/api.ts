export async function getUser(id: string): Promise<unknown> {
  const res = await fetch(`/users/${id}`);
  return res.json();
}
