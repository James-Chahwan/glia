import { Kafka } from 'kafkajs';

const kafka = new Kafka({ clientId: 'billing-worker', brokers: ['localhost:9092'] });
const consumer = kafka.consumer({ groupId: 'billing' });

export async function start(): Promise<void> {
  await consumer.subscribe({ topic: 'orders', fromBeginning: true });
  await consumer.subscribe({ topic: 'payments', fromBeginning: true });
}
