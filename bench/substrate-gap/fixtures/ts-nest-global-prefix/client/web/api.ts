export async function listUsers() {
  return fetch("/svc/backoffice/users/all");
}

export async function createUser(body: string) {
  return fetch("/svc/backoffice/users", { method: "POST", body });
}

export async function version() {
  return fetch("/svc/version");
}

export async function health() {
  return fetch("/health");
}
