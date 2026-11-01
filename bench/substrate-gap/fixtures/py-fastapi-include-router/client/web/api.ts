export async function listUsers() {
  return fetch("/svc/core/users");
}

export async function createUser(body: string) {
  return fetch("/svc/core/users", { method: "POST", body });
}

export async function filterFlows(body: string) {
  return fetch("/svc/core/v1/flows/filter", { method: "POST", body });
}
