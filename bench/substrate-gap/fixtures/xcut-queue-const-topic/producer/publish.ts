import { Kafka } from 'kafkajs';
import { ORDERS_TOPIC, Topics } from './topics';

const kafka = new Kafka({ clientId: 'orders-api', brokers: ['localhost:9092'] });
const producer = kafka.producer();

export async function publishOrder(order: { id: string }): Promise<void> {
  await producer.send({ topic: ORDERS_TOPIC, messages: [{ value: JSON.stringify(order) }] });
}

export async function publishPayment(payment: { id: string }): Promise<void> {
  await producer.send({ topic: Topics.PAYMENTS, messages: [{ value: JSON.stringify(payment) }] });
}

// A parameter: nothing in this repo names the topic, so no node may claim one.
export async function publishTo(topic: string, body: string): Promise<void> {
  await producer.send({ topic: topic, messages: [{ value: body }] });
}
