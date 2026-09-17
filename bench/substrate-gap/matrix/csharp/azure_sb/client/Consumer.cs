using System.Threading.Tasks;
using Azure.Messaging.ServiceBus;

namespace Shop.Workers
{
    public class OrderConsumer
    {
        private readonly ServiceBusClient _client;

        public OrderConsumer(ServiceBusClient client) => _client = client;

        public async Task StartAsync()
        {
            ServiceBusProcessor processor = _client.CreateProcessor("orders", new ServiceBusProcessorOptions());
            processor.ProcessMessageAsync += args => args.CompleteMessageAsync(args.Message);
            processor.ProcessErrorAsync += args => Task.CompletedTask;
            await processor.StartProcessingAsync();
        }
    }
}
