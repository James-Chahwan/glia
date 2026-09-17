using System.Threading.Tasks;
using Azure.Messaging.ServiceBus;

namespace Shop.Messaging
{
    public class OrderProducer
    {
        private readonly ServiceBusClient _client;

        public OrderProducer(ServiceBusClient client) => _client = client;

        public async Task PublishAsync(string payload)
        {
            await using ServiceBusSender sender = _client.CreateSender("orders");
            await sender.SendMessageAsync(new ServiceBusMessage(payload));
        }
    }
}
