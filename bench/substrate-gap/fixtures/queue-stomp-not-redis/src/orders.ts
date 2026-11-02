import Redis from 'ioredis';

const sub = new Redis();

export function listenForOrders(handle: (id: string) => void): void {
  sub.subscribe('orders', (err) => {
    if (err) throw err;
  });
  sub.on('message', (_channel: string, id: string) => handle(id));
}
