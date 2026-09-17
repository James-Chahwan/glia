import { Message, PubSub } from '@google-cloud/pubsub';

const pubsub = new PubSub();

export function listen(handle: (m: Message) => void): void {
  const sub = pubsub.subscription('orders-worker');
  sub.on('message', handle);
}
