import { PubSub } from '@google-cloud/pubsub';

const pubsub = new PubSub();

export async function publishOrder(data: Buffer): Promise<string> {
  return pubsub.topic('orders').publishMessage({ data });
}
