export function getUser(id) {
  return fetch('/api/users/' + id);
}
