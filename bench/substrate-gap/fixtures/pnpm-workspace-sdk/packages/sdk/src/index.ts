export interface UserUpdate {
  name: string;
}

export async function updateMyUser(body: UserUpdate): Promise<Response> {
  return fetch('/api/users/me', {
    method: 'PUT',
    body: JSON.stringify(body),
  });
}
