import { Kafka } from 'kafkajs';

const kafka = new Kafka({ clientId: 'shop', brokers: ['localhost:9092'] });
const producer = kafka.producer();

export async function publishOrder(order: { id: string }): Promise<void> {
  await producer.send({
    topic: 'orders',
    messages: [{ value: JSON.stringify(order) }],
  });
}

export async function publishPayment(payment: { id: string }): Promise<void> {
  await producer.send({
    topic: 'payments',
    messages: [{ value: JSON.stringify(payment) }],
  });
}
