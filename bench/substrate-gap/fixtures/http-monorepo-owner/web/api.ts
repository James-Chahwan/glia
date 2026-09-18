import axios from 'axios';

const USERS_API = 'http://users-svc:8080';

export async function ping() {
  return axios.get(`${USERS_API}/health`);
}
