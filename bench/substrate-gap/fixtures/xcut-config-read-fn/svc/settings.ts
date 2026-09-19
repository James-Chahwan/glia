import axios from 'axios';

const MODE = process.env.RUNTIME_MODE;

export async function sendNotice(order: unknown) {
  const url = process.env.NOTIFY_URL;
  await axios.post(`${url}/notify`, { order, mode: MODE });
}
