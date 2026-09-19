import { Kafka } from 'kafkajs';
import amqp from 'amqplib';
import { Worker } from 'bullmq';
import { onOrder } from './handlers';

const kafka = new Kafka({ clientId: 'svc', brokers: ['k:9092'] });
const consumer = kafka.consumer({ groupId: 'svc' });

export async function handlePayment({ message }) {
  await settle(message.value);
}

export async function settle(v) { return v; }

export async function start() {
  await consumer.connect();
  await consumer.subscribe({ topic: 'payments' });
  await consumer.run({ eachMessage: handlePayment });
}

export async function startRabbit() {
  const conn = await amqp.connect('amqp://x');
  const channel = await conn.createChannel();
  channel.consume('orders', onOrder);
}

export function startJobs() {
  return new Worker('emails', (job) => sendEmail(job));
}

export async function sendEmail(job) { return job; }
