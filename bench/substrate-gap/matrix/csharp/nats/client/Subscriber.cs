using NATS.Client.Core;

public class Subscriber
{
    public async Task Run(NatsConnection nc)
    {
        await foreach (var msg in nc.SubscribeAsync<string>("orders"))
        {
            Console.WriteLine(msg.Data);
        }
    }
}
