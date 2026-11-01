export async function listIssues(slug: string) {
  return fetch(`/api/workspaces/${slug}/issues/`);
}

export async function listThings() {
  return fetch("/api/v2/things/");
}

export async function login(body: string) {
  return fetch("/auth/login/", { method: "POST", body });
}
