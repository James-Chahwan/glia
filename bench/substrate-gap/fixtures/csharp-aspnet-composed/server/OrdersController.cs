// Conventional ASP.NET attribute routing: the controller carries the shared
// template (with the [controller] token) and each action carries a RELATIVE
// one. The composed route is what a client actually calls, so it is what the
// graph must emit — absolute, so index_route_node will index it.
using Microsoft.AspNetCore.Mvc;

namespace Shop.Controllers
{
    [ApiController]
    [Route("api/v2/[controller]")]
    public class OrdersController : ControllerBase
    {
        [HttpGet("{id}")]
        public Order GetOrder(int id)
        {
            return null;
        }

        [HttpPost]
        public Order Create(Order o)
        {
            return null;
        }

        // Absolute action template: ASP.NET semantics say a leading `/`
        // OVERRIDES the controller prefix. Control inside the fixture.
        [HttpDelete("/admin/orders/{id}")]
        public void Purge(int id)
        {
        }
    }
}
