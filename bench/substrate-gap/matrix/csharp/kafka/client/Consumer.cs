using System.Threading;
using Confluent.Kafka;

namespace Shop.Workers
{
    public class OrderConsumer
    {
        private readonly IConsumer<string, string> _consumer;

        public OrderConsumer(IConsumer<string, string> consumer) => _consumer = consumer;

        public void Run(CancellationToken ct)
        {
            _consumer.Subscribe("orders");
            ConsumeResult<string, string> cr = _consumer.Consume(ct);
            System.Console.WriteLine(cr.Message.Value);
        }
    }
}
