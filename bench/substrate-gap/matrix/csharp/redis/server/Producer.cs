using System.Threading.Tasks;
using StackExchange.Redis;

namespace Shop.Messaging
{
    public class OrderProducer
    {
        private readonly IDatabase _db;

        public OrderProducer(IConnectionMultiplexer redis) => _db = redis.GetDatabase();

        public Task<long> PublishAsync(string payload) => _db.ListLeftPushAsync("orders", payload);
    }
}
