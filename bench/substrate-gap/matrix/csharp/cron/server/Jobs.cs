namespace Billing;

public interface IInvoiceService
{
    void SendReminders();
}

public class InvoiceService : IInvoiceService
{
    public void SendReminders() { }
}

public static class Cleaner
{
    public static void Run() { }
}
