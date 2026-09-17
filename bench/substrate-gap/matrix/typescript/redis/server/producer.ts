import Redis from 'ioredis';

const redis = new Redis({ host: 'localhost', port: 6379 });

export async function publishOrder(payload: string): Promise<void> {
  await redis.lpush('orders', payload);
}
