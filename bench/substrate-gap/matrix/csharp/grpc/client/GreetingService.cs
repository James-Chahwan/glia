using Demo;
using Grpc.Net.ClientFactory;

namespace Demo.Web;

public class GreetingService
{
    private readonly Greeter.GreeterClient _client;

    public GreetingService(Greeter.GreeterClient client)
    {
        _client = client;
    }

    public async Task<string> GreetAsync(string name)
    {
        var reply = await _client.SayHelloAsync(new HelloRequest { Name = name });
        return reply.Message;
    }
}
