using Microsoft.AspNetCore.Mvc;
using Swashbuckle.AspNetCore.Annotations;

namespace Shop.Controllers;

[ApiController]
[Route("api/users")]
public class UsersController : ControllerBase
{
    [HttpGet("{id}")]
    [SwaggerOperation(Summary = "Get a user", OperationId = "GetUser")]
    [ProducesResponseType(typeof(UserDto), 200)]
    [ProducesResponseType(StatusCodes.Status404NotFound)]
    public IActionResult Get(string id) => Ok();

    [HttpDelete("{id}")]
    public IActionResult Delete(string id) => NoContent();
}
