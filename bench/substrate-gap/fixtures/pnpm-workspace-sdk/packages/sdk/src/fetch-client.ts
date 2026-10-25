export async function getServerVersion(): Promise<Response> {
  return fetch('/api/server/version');
}
