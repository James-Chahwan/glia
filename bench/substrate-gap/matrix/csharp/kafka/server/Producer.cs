using Confluent.Kafka;

namespace Shop.Messaging
{
    public class OrderProducer
    {
        private readonly IProducer<string, string> _producer;

        public OrderProducer(IProducer<string, string> producer) => _producer = producer;

        public void Publish(string payload)
        {
            _producer.Produce("orders", new Message<string, string> { Value = payload });
        }
    }
}
