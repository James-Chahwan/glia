import Redis from 'ioredis';

const redis = new Redis({ host: 'localhost', port: 6379 });

function handle(payload: string): void {
  console.log(payload);
}

export async function run(): Promise<void> {
  for (;;) {
    const item = await redis.blpop('orders', 0);
    if (item) handle(item[1]);
  }
}
