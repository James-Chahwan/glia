namespace Shop.Api;

public interface IBillingService
{
    void Charge();
}

public class BillingService : IBillingService
{
    public void Charge() { }
}
