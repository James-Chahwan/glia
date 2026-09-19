using Hangfire;

namespace Billing;

public class Startup
{
    public void Configure()
    {
        RecurringJob.AddOrUpdate<IInvoiceService>("invoices", x => x.SendReminders(), Cron.Daily);
        RecurringJob.AddOrUpdate("cleanup", () => Cleaner.Run(), "*/10 * * * *");
    }
}
