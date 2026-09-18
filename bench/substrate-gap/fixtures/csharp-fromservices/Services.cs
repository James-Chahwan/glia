namespace Shop.Services
{
    public interface IReportService
    {
        string Build();
    }

    public class ReportService : IReportService
    {
        public string Build()
        {
            return "r";
        }
    }
}
