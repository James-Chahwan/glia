using System.Threading.Tasks;
using Grpc.Core;
using GreeterApi;

namespace GreeterApi.Services
{
    public class GreeterService : Greeter.GreeterBase
    {
        public override Task<HelloReply> SayHello(HelloRequest request, ServerCallContext context)
        {
            return Task.FromResult(new HelloReply { Message = "hello " + request.Name });
        }
    }
}
