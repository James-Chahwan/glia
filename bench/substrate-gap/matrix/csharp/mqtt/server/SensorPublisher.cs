using MQTTnet;
using MQTTnet.Client;

public class SensorPublisher
{
    public async Task Send(IMqttClient client)
    {
        var msg = new MqttApplicationMessageBuilder().WithTopic("sensors/temp").WithPayload("21").Build();
        await client.PublishAsync(msg);
    }
}
