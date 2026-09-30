using Amazon.SQS;
using Amazon.SQS.Model;

public class Enqueuer
{
    public async Task Send(IAmazonSQS sqs, string body)
    {
        await sqs.SendMessageAsync(new SendMessageRequest { QueueUrl = "https://sqs.us-east-1.amazonaws.com/123/orders", MessageBody = body });
    }
}
