// Refit: the HTTP contract IS the interface. Each method carries a verb
// attribute whose single string argument is the request path template, so the
// C# client side of a cross-service call is expressible entirely in attributes
// — there is no call site to walk. `[Get("/api/users/{id}")]` must become
// ENDPOINT `GET /api/users/{id}`, which normalise_http_path collapses to
// `/api/users/{}` and pairs with the Go chi route in ../server.
using System.Threading.Tasks;
using Refit;

namespace Shop.Clients
{
    public interface IUserApi
    {
        [Get("/api/users/{id}")]
        Task<User> GetUserAsync(int id);

        [Post("/api/users")]
        Task<User> CreateUserAsync([Body] User user);
    }

    // Precision control: the SAME attribute spelling on a class must stay
    // inert. Refit and RestEase contracts are always interfaces, so gating on
    // interface membership is what keeps an unrelated `[Get]` attribute (and
    // ASP.NET's own attribute vocabulary) from minting phantom endpoints.
    public class NotARefitClient
    {
        [Get("/api/ghost")]
        public void Ghost() { }
    }

    public class User { }
}
