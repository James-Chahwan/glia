export async function loadUsers() {
  return fetch('/users');
}

export async function addUser(u: unknown) {
  return fetch('/users', { method: 'POST', body: JSON.stringify(u) });
}

export async function dropUser(id: string) {
  return fetch(`/users/${id}`, { method: 'DELETE' });
}
