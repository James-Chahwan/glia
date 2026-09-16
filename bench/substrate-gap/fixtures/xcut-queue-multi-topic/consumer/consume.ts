import { Kafka } from 'kafkajs';

const kafka = new Kafka({ clientId: 'orders-worker', brokers: ['localhost:9092'] });
const consumer = kafka.consumer({ groupId: 'orders-group' });

export async function runOrdersWorker(): Promise<void> {
  await consumer.subscribe({ topic: 'orders', fromBeginning: true });
  await consumer.run({
    eachMessage: async ({ message }) => {
      handleOrder(message.value);
    },
  });
}

function handleOrder(value: unknown): void {
  void value;
}
