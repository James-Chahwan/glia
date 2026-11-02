import axios from 'axios';

export interface User {
  id: number;
  name: string;
}

export interface Account {
  id: number;
  plan: string;
}

const GATEWAY = process.env.GATEWAY_URL;

export async function listUsers() {
  return axios.get<User[]>(`${GATEWAY}/users`);
}

export async function listAccounts() {
  return axios.get<Account[]>('/api/accounts');
}
