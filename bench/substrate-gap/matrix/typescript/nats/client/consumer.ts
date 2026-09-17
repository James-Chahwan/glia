import { connect, StringCodec } from 'nats';

const sc = StringCodec();

export async function run(): Promise<void> {
  const nc = await connect({ servers: 'localhost:4222' });
  const sub = nc.subscribe('orders');
  for await (const m of sub) {
    console.log(sc.decode(m.data));
  }
}
