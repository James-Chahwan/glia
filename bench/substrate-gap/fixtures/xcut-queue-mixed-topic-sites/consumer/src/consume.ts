import { Kafka } from 'kafkajs';

const kafka = new Kafka({ clientId: 'audit-worker', brokers: ['kafka:9092'] });
const consumer = kafka.consumer({ groupId: 'audit-group' });

export async function runAuditWorker(): Promise<void> {
  await consumer.subscribe({ topic: 'audit.events' });
  await consumer.run({
    eachMessage: async ({ message }) => record(message.value),
  });
}

function record(value: unknown): void {
  void value;
}
