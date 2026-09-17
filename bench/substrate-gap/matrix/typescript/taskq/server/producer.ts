import { Queue } from 'bullmq';

const queue = new Queue('orders', { connection: { host: 'localhost', port: 6379 } });

export async function checkout(orderId: string): Promise<void> {
  await queue.add('process-order', { orderId });
}
