using GraphQL;
using GraphQL.Client.Http;

public class BookClient
{
    public async Task Load(GraphQLHttpClient client)
    {
        var request = new GraphQLRequest { Query = "query { books { title } }" };
        await client.SendQueryAsync<object>(request);
    }
}
