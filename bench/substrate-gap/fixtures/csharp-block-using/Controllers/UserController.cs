namespace Shop.Controllers
{
    using Shop.Models;
    using Shop.Services.Billing;

    public class UserController
    {
        public int Show(int id)
        {
            var u = new User();
            return u.Find(id);
        }
    }
}
