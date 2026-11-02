// Positive control in the composed shape: a controller [Route] template, a
// templated verb attribute and a bare verb attribute inheriting the template.
using Microsoft.AspNetCore.Mvc;

namespace Boards.Api
{
    [ApiController]
    [Route("api/cards")]
    public class CardsController : ControllerBase
    {
        [HttpGet("{id}")]
        public IActionResult GetCard(int id)
        {
            return Ok(id);
        }

        [HttpPost]
        public IActionResult CreateCard()
        {
            return Ok();
        }
    }
}
