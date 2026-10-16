import { Kafka } from 'kafkajs';

const kafka = new Kafka({ clientId: 'shop', brokers: ['kafka:9092'] });
const producer = kafka.producer();

export async function audit(e: { id: string }): Promise<void> {
  await producer.send({ topic: 'audit.events', messages: [{ value: JSON.stringify(e) }] });
}

export async function forward(topic: string, value: string): Promise<void> {
  await producer.send({ topic: topic, messages: [{ value }] });
}

export async function route(cfg: { topic: string }, value: string): Promise<void> {
  await producer.send({ topic: cfg.topic, messages: [{ value }] });
}
