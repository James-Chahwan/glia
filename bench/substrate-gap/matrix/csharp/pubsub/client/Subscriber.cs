using Google.Cloud.PubSub.V1;

public class Subscriber
{
    public async Task Run()
    {
        var sub = await SubscriberClient.CreateAsync(SubscriptionName.FromProjectSubscription("shop", "orders"));
        await sub.StartAsync((msg, ct) => Task.FromResult(SubscriberClient.Reply.Ack));
    }
}
