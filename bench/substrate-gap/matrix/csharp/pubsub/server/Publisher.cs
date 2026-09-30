using Google.Cloud.PubSub.V1;

public class Publisher
{
    public async Task Send(string body)
    {
        var pub = await PublisherClient.CreateAsync(TopicName.FromProjectTopic("shop", "orders"));
        await pub.PublishAsync(body);
    }
}
