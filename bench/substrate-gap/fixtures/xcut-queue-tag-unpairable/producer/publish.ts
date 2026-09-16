import { Kafka } from 'kafkajs';

const kafka = new Kafka({ clientId: 'orders-api', brokers: ['localhost:9092'] });
const producer = kafka.producer();

// The topic is an environment variable, so NO literal is readable here. This is
// the shape that used to collapse to a topic-agnostic `queue_producer:kafka`.
const topic = process.env.ORDERS_TOPIC!;

export async function publishOrder(order: { id: string }): Promise<void> {
  await producer.send({ topic, messages: [{ value: JSON.stringify(order) }] });
}
