export async function createAuthority(data: unknown) {
  return fetch('/authority/createAuthority', { method: 'POST', body: JSON.stringify(data) });
}

export async function getAuthorityList() {
  return fetch('/authority/getAuthorityList', { method: 'POST' });
}

export async function addMenu(data: unknown) {
  return fetch('/menu/addMenu', { method: 'POST', body: JSON.stringify(data) });
}

export async function getLogList() {
  return fetch('/sysLog/getLogList');
}
