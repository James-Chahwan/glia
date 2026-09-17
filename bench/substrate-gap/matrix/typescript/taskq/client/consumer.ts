import { Worker, Job } from 'bullmq';

async function processOrder(job: Job): Promise<void> {
  console.log(job.data.orderId);
}

export const worker = new Worker('orders', processOrder, {
  connection: { host: 'localhost', port: 6379 },
});
