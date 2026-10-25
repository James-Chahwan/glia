using System.Threading.Tasks;
using Grpc.Core;
using Shop;

namespace cartservice.services
{
    public class CartService : Shop.CartService.CartServiceBase
    {
        public override Task<Empty> AddItem(AddItemRequest request, ServerCallContext context)
        {
            return Task.FromResult(new Empty());
        }

        public override Task<Cart> GetCart(GetCartRequest request, ServerCallContext context)
        {
            return Task.FromResult(new Cart { UserId = request.UserId });
        }
    }
}
