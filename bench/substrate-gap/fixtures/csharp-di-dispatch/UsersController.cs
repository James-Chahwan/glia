using Microsoft.AspNetCore.Mvc;
using Shop.Services;

namespace Shop.Controllers
{
    [ApiController]
    [Route("api/users")]
    public class UsersController : ControllerBase
    {
        private readonly IUserService _userService;

        public UsersController(IUserService userService)
        {
            _userService = userService;
        }

        [HttpGet("{id}")]
        public User GetUser(int id)
        {
            return _userService.GetById(id);
        }
    }
}
