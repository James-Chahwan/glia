export async function listUsers() {
  return fetch("/svc/users/list");
}

export async function createUser(body: string) {
  return fetch("/svc/users", { method: "POST", body });
}

export async function listItems() {
  return fetch("/svc/items/all");
}

export async function ping() {
  return fetch("/svc/health/ping");
}
