import amqp, { ConsumeMessage } from 'amqplib';

export async function run(): Promise<void> {
  const conn = await amqp.connect('amqp://localhost');
  const channel = await conn.createChannel();
  await channel.assertExchange('shop', 'direct');
  await channel.assertQueue('orders');
  await channel.bindQueue('orders', 'shop', 'orders');
  await channel.consume('orders', (msg: ConsumeMessage | null) => {
    if (msg) channel.ack(msg);
  });
}
