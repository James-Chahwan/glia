export async function updateMe(body: unknown) {
  return fetch('/users/me', { method: 'PUT', body: JSON.stringify(body) });
}

export async function sharedAlbums() {
  return fetch('/users/albums/shared');
}

export async function adminStats() {
  return fetch('/admin/stats');
}
