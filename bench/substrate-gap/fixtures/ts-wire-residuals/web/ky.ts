import ky from 'ky';
import { User } from './models';

export async function listUsers() {
  return ky.get('/users').json<User[]>();
}
