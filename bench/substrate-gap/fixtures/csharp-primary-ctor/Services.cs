namespace Shop.Services
{
    public interface IOrderService
    {
        string Place();
    }

    public class OrderService : IOrderService
    {
        public string Place()
        {
            return "ok";
        }
    }
}
