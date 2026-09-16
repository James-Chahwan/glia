export async function loadDashboard() {
  return fetch('/dashboard').then((r) => r.json());
}
