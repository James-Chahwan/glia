function base(): string {
  return '/api';
}

function buildUrl(id: string) {
  return base() + '/r/' + id;
}

export async function loadReport(id: string) {
  return fetch(buildUrl(id));
}
