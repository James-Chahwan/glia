using System.Text;
using RabbitMQ.Client;

namespace Shop.Messaging
{
    public class OrderProducer
    {
        private readonly IModel _channel;

        public OrderProducer(IModel channel) => _channel = channel;

        public void Publish(string payload)
        {
            _channel.QueueDeclare(queue: "orders", durable: true, exclusive: false, autoDelete: false, arguments: null);
            _channel.BasicPublish(exchange: "", routingKey: "orders", basicProperties: null, body: Encoding.UTF8.GetBytes(payload));
        }
    }
}
