// Raw HttpClient: the HTTP contract lives at the CALL SITE, not in an
// attribute. Each `_http.<Verb>Async("<path>", …)` invocation inside a method
// body must become an ENDPOINT hung off the enclosing method. The precision
// question is that `GetAsync` is also IDistributedCache / StackExchange.Redis
// vocabulary — `url_to_path` returning None for a non-path string is the only
// thing keeping those out, so this fixture ships one of each.
using System.Net.Http;
using System.Net.Http.Json;
using System.Threading.Tasks;

namespace Shop.Clients
{
    public class OrderClient
    {
        private readonly HttpClient _http;
        private readonly ICache _cache;

        public OrderClient(HttpClient http, ICache cache)
        {
            _http = http;
            _cache = cache;
        }

        // Interpolated path -> Medium confidence, `${…}` segment so
        // normalise_http_path collapses it to `/api/users/{}`.
        public async Task<string> GetUserAsync(int id)
        {
            var cached = await _cache.GetAsync("user:42");
            var res = await _http.GetAsync($"/api/users/{id}");
            return await res.Content.ReadAsStringAsync();
        }

        // Literal path -> Strong confidence.
        public async Task<string> CreateAsync(object u)
        {
            var res = await _http.PostAsJsonAsync("/api/users", u);
            return await res.Content.ReadAsStringAsync();
        }
    }

    public interface ICache
    {
        Task<string> GetAsync(string key);
    }
}
