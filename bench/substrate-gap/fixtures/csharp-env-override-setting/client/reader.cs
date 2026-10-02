using Microsoft.Extensions.Configuration;

namespace Payments;

public class StripeGateway
{
    private readonly string _apiKey;
    private readonly int _retries;

    public StripeGateway(IConfiguration configuration)
    {
        _apiKey = configuration["Stripe:SecretKey"];
        _retries = configuration.GetValue<int>("Retry:Max");
    }
}
