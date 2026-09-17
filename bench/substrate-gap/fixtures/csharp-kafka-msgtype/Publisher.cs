using System.Threading.Tasks;
using Confluent.Kafka;
using Demo.Events;

namespace Demo.Ordering;

public class OrderPublisher
{
    private readonly IProducer<Null, OrderCreated> _producer;

    public OrderPublisher()
    {
        var config = new ProducerConfig { BootstrapServers = "kafka:9092" };
        _producer = new ProducerBuilder<Null, OrderCreated>(config).Build();
    }

    public async Task PublishAsync(OrderCreated evt)
    {
        await _producer.ProduceAsync("orders", new Message<Null, OrderCreated> { Value = evt });
    }
}
