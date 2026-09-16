using System.Threading;
using Confluent.Kafka;

namespace Shop.Orders;

public sealed class OrderConsumer
{
    public void Run(ConsumerConfig config, CancellationToken ct)
    {
        var consumer = new ConsumerBuilder<Ignore, string>(config).Build();
        consumer.Subscribe("orders");
        while (!ct.IsCancellationRequested)
        {
            var result = consumer.Consume(ct);
            Handle(result.Message.Value);
        }
    }

    private void Handle(string value)
    {
    }
}
