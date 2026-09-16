import { Kafka } from 'kafkajs';

const kafka = new Kafka({ clientId: 'payments-worker', brokers: ['localhost:9092'] });
const consumer = kafka.consumer({ groupId: 'payments-group' });

export async function runPaymentsWorker(): Promise<void> {
  await consumer.subscribe({ topic: 'payments', fromBeginning: true });
  await consumer.run({
    eachMessage: async ({ message }) => {
      handlePayment(message.value);
    },
  });
}

function handlePayment(value: unknown): void {
  void value;
}
