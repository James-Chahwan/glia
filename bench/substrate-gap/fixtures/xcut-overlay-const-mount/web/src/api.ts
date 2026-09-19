import axios from 'axios';

const GATEWAY = process.env.GATEWAY_URL;

export async function listUsers() {
  return axios.get(`${GATEWAY}/users`);
}

export async function listOrders() {
  return axios.get('/orders-svc/orders');
}
