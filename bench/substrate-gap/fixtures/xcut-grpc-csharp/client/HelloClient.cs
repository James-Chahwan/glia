using System.Threading.Tasks;
using Grpc.Net.Client;
using GreeterApi;

namespace Storefront.Clients
{
    public class HelloClient
    {
        public async Task<string> FetchGreeting(string name)
        {
            using var channel = GrpcChannel.ForAddress("https://localhost:5001");
            var client = new Greeter.GreeterClient(channel);
            var reply = await client.SayHelloAsync(new HelloRequest { Name = name });
            return reply.Message;
        }
    }
}
