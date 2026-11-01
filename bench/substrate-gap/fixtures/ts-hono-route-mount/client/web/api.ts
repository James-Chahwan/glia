export async function uploadPdf(body: FormData) {
  return fetch("/svc/files/upload-pdf", { method: "POST", body });
}

export async function getItem(id: string) {
  return fetch(`/svc/files/item/${id}`);
}

export async function getSession() {
  return fetch("/svc/auth/session");
}

export async function signOut() {
  return fetch("/svc/auth/sign-out", { method: "POST" });
}
