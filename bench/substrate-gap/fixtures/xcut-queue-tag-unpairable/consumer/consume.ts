import { Kafka } from 'kafkajs';
import { config } from './config';

const kafka = new Kafka({ clientId: 'billing-worker', brokers: ['localhost:9092'] });
const consumer = kafka.consumer({ groupId: 'billing' });

// Topic comes from config, not a literal — an UNRELATED service to the producer
// next door. Nothing about these two files says they share a queue.
export async function start(): Promise<void> {
  await consumer.subscribe({ topic: config.topic, fromBeginning: true });
}
