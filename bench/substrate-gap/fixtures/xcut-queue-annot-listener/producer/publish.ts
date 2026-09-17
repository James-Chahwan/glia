import { Kafka } from 'kafkajs';
import { SQSClient, SendMessageCommand } from '@aws-sdk/client-sqs';

const kafka = new Kafka({ clientId: 'checkout', brokers: ['localhost:9092'] });
const producer = kafka.producer();
const sqs = new SQSClient({ region: 'us-east-1' });

export async function publishOrder(order: { id: string }): Promise<void> {
  await producer.send({ topic: 'orders', messages: [{ value: JSON.stringify(order) }] });
}

export async function requestRefund(orderId: string): Promise<void> {
  await sqs.send(new SendMessageCommand({
    QueueUrl: 'https://sqs.us-east-1.amazonaws.com/123456789012/refunds',
    MessageBody: orderId,
  }));
}
