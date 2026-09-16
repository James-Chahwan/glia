using System.Threading.Tasks;
using Confluent.Kafka;

namespace Shop.Orders;

public sealed class OrderProducer
{
    private readonly IProducer<Null, string> _producer;

    public OrderProducer(IProducer<Null, string> producer)
    {
        _producer = producer;
    }

    public async Task PublishAsync(string payload)
    {
        await _producer.ProduceAsync("orders", new Message<Null, string> { Value = payload });
    }
}
