using Shop.Services;
using System.Threading.Tasks;

namespace Shop.Controllers
{
    public class UsersController
    {
        public Task<string> Get() => Task.FromResult(new UserService().Name());
    }
}
