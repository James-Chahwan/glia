export async function loadUsers(): Promise<unknown> {
  const res = await fetch('/users');
  return res.json();
}
