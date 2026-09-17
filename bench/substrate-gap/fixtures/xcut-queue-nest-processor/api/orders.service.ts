import { Injectable } from '@nestjs/common';
import { Queue } from 'bullmq';

const emailQueue = new Queue('emails', { connection: { host: 'localhost', port: 6379 } });

@Injectable()
export class OrdersService {
  async placeOrder(to: string): Promise<void> {
    await emailQueue.add('welcome', { to });
  }
}
