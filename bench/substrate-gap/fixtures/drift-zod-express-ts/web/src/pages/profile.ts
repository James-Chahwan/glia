import { fetchUser } from '../lib/users';

export async function renderProfile(id: string): Promise<string> {
  const { userName, email_address } = await fetchUser(id);
  return `${userName} <${email_address}>`;
}
