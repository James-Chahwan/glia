import { Kafka } from 'kafkajs';

const kafka = new Kafka({ clientId: 'shop', brokers: ['localhost:9092'] });
const producer = kafka.producer();

export async function publishOrder(payload: string): Promise<void> {
  await producer.connect();
  await producer.send({ topic: 'orders', messages: [{ value: payload }] });
}
