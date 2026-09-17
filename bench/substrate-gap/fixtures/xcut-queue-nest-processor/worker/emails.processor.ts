import { Processor, WorkerHost } from '@nestjs/bullmq';
import { Job } from 'bullmq';

@Processor('emails')
export class EmailsProcessor extends WorkerHost {
  async process(job: Job<{ to: string }>): Promise<void> {
    await sendWelcome(job.data.to);
  }
}

async function sendWelcome(to: string): Promise<void> {
  console.log(`welcome ${to}`);
}
