using MQTTnet;
using MQTTnet.Client;

public class SensorSubscriber
{
    public async Task Listen(IMqttClient client)
    {
        await client.SubscribeAsync("sensors/temp");
    }
}
