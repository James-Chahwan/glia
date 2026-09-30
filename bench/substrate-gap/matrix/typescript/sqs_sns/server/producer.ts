import { SQSClient, SendMessageCommand } from "@aws-sdk/client-sqs";

const sqs = new SQSClient({});

export async function enqueue(body: string) {
  return sqs.send(new SendMessageCommand({ QueueUrl: "https://sqs.us-east-1.amazonaws.com/123/orders", MessageBody: body }));
}
