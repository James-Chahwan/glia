using Amazon.SQS;
using Amazon.SQS.Model;

public class Poller
{
    public async Task Poll(IAmazonSQS sqs)
    {
        await sqs.ReceiveMessageAsync(new ReceiveMessageRequest { QueueUrl = "https://sqs.us-east-1.amazonaws.com/123/orders" });
    }
}
