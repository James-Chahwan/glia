// An [ApiController] with no controller [Route]: every action pairs a
// template-less verb attribute with its own [Route] template. ASP.NET reads
// each pair as ONE route (the verb constrains the template), so there is no
// `GET /` route and no verb-less `ANY /boards/{board}`.
using Microsoft.AspNetCore.Mvc;

namespace Boards.Api
{
    [ApiController]
    public class BoardsController : ControllerBase
    {
        [HttpGet]
        [Route("boards/{board}")]
        public IActionResult GetBoard(string board)
        {
            return Ok(board);
        }

        [HttpPut]
        [Route("boards/{board}/")]
        public IActionResult PutBoard(string board)
        {
            return Ok(board);
        }

        [HttpDelete]
        [Route("boards/{board}/")]
        public IActionResult DeleteBoard(string board)
        {
            return NoContent();
        }

        [HttpGet]
        [Route("boards/{board}/columns/completion")]
        public IActionResult GetColumnsCompletion(string board)
        {
            return Ok(board);
        }
    }
}
