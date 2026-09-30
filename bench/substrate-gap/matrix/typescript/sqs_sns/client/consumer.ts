import { SQSClient, ReceiveMessageCommand } from "@aws-sdk/client-sqs";

const sqs = new SQSClient({});

export async function poll() {
  return sqs.send(new ReceiveMessageCommand({ QueueUrl: "https://sqs.us-east-1.amazonaws.com/123/orders" }));
}
