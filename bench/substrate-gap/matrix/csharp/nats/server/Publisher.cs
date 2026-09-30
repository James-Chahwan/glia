using NATS.Client.Core;

public class Publisher
{
    public async Task Send(NatsConnection nc, string body)
    {
        await nc.PublishAsync("orders", body);
    }
}
