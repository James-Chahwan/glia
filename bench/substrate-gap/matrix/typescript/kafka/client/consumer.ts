import { Kafka, EachMessagePayload } from 'kafkajs';

const kafka = new Kafka({ clientId: 'worker', brokers: ['localhost:9092'] });
const consumer = kafka.consumer({ groupId: 'workers' });

async function handle({ message }: EachMessagePayload): Promise<void> {
  console.log(message.value?.toString());
}

export async function run(): Promise<void> {
  await consumer.connect();
  await consumer.subscribe({ topic: 'orders', fromBeginning: true });
  await consumer.run({ eachMessage: handle });
}
