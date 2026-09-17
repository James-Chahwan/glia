import { connect, StringCodec } from 'nats';

const sc = StringCodec();

export async function publishOrder(payload: string): Promise<void> {
  const nc = await connect({ servers: 'localhost:4222' });
  nc.publish('orders', sc.encode(payload));
  await nc.drain();
}
