using System.Threading.Tasks;
using StackExchange.Redis;

namespace Shop.Workers
{
    public class OrderConsumer
    {
        private readonly IDatabase _db;

        public OrderConsumer(IConnectionMultiplexer redis) => _db = redis.GetDatabase();

        public async Task PollAsync()
        {
            RedisValue payload = await _db.ListRightPopAsync("orders");
            System.Console.WriteLine(payload);
        }
    }
}
