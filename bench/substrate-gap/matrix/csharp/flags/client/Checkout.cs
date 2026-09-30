using LaunchDarkly.Sdk;
using LaunchDarkly.Sdk.Server;

public class Checkout
{
    public string Variant(LdClient client, string userKey)
        => client.BoolVariation("new-checkout", Context.New(userKey), false) ? "new" : "legacy";
}
