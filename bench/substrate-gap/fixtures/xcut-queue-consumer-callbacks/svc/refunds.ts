import { Kafka } from 'kafkajs';

export class RefundsConsumer {
  private consumer = new Kafka({ clientId: 'r', brokers: [] }).consumer({ groupId: 'r' });

  async start() {
    await this.consumer.subscribe({ topic: 'refunds' });
    await this.consumer.run({ eachMessage: this.handle.bind(this) });
  }

  async handle(payload) { return payload; }
}
