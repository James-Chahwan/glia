const api = makeClient();

export function request(method: string, path: string) {
  return fetch(path, { method });
}

export async function loadUsers() {
  return request('GET', '/users');
}

export async function createUser(u: unknown) {
  return request("POST", "/users");
}

// request('GET', '/legacy');

export async function dyn(v: string, p: string) {
  return request(v, p);
}

export async function listOrders() {
  return api.get('/orders');
}
