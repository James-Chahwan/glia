using Microsoft.AspNetCore.Mvc;

namespace Shop.Api;

[ApiController]
[Route("api/users")]
public class UsersController : ControllerBase
{
    [HttpGet]
    public IActionResult Get() => Ok();
}
