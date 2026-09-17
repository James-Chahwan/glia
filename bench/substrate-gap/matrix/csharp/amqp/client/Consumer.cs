using RabbitMQ.Client;
using RabbitMQ.Client.Events;

namespace Shop.Workers
{
    public class OrderConsumer
    {
        private readonly IModel _channel;

        public OrderConsumer(IModel channel) => _channel = channel;

        public void Run()
        {
            _channel.QueueDeclare(queue: "orders", durable: true, exclusive: false, autoDelete: false, arguments: null);
            var consumer = new EventingBasicConsumer(_channel);
            consumer.Received += (model, ea) => System.Console.WriteLine(System.Text.Encoding.UTF8.GetString(ea.Body.ToArray()));
            _channel.BasicConsume(queue: "orders", autoAck: true, consumer: consumer);
        }
    }
}
