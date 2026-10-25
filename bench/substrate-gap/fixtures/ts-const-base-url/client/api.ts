import { ApiBase, Routes } from './endpoints';

export async function listUsers() {
  return fetch(`${ApiBase.Users}/list`);
}

export async function listOrders() {
  return fetch(`${Routes.orders}/recent`);
}
