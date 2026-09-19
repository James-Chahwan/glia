import { Kafka } from 'kafkajs';

const kafka = new Kafka({ clientId: 'svc', brokers: ['localhost:9092'] });
const producer = kafka.producer();
const consumer = kafka.consumer({ groupId: 'svc' });

export async function publishOrder(order: { id: string }): Promise<void> {
  await producer.send({
    topic: 'orders',
    messages: [{ value: JSON.stringify(order) }],
  });
}

export async function listen(): Promise<void> {
  await consumer.subscribe({ topic: 'payments', fromBeginning: true });
  await consumer.run({
    eachMessage: async ({ message }) => {
      void message;
    },
  });
}
