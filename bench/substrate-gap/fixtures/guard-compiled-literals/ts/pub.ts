import { connect } from 'nats';

const t = `nc.publish('ghost.ts', m)`;

export async function send(m: Uint8Array) {
  const nc = await connect();
  nc.publish('orders.ts', m);
}
