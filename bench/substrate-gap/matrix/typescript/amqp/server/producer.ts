import amqp from 'amqplib';

export async function publishOrder(payload: string): Promise<void> {
  const conn = await amqp.connect('amqp://localhost');
  const channel = await conn.createChannel();
  await channel.assertExchange('shop', 'direct');
  channel.publish('shop', 'orders', Buffer.from(payload));
}
