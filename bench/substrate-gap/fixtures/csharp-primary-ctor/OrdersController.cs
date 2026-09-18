using Microsoft.AspNetCore.Mvc;
using Shop.Services;

namespace Shop.Controllers
{
    [ApiController]
    [Route("api/orders")]
    public class OrdersController(IOrderService orders, int pageSize) : ControllerBase
    {
        [HttpGet]
        public string Place()
        {
            return orders.Place();
        }
    }
}
