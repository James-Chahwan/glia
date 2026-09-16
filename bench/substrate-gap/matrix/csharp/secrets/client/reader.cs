using Microsoft.Extensions.Configuration;

namespace Payments;

public class StripeGateway
{
    private readonly string _apiKey;

    public StripeGateway(IConfiguration configuration)
    {
        _apiKey = configuration["Stripe:SecretKey"];
    }
}
