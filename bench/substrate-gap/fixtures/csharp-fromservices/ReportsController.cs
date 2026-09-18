using Microsoft.AspNetCore.Mvc;
using Shop.Services;

namespace Shop.Controllers
{
    [ApiController]
    public class ReportsController : ControllerBase
    {
        [HttpGet("reports")]
        public string Get([FromServices] IReportService reports, int page)
        {
            return reports.Build();
        }
    }
}
