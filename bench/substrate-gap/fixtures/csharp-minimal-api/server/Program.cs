// ASP.NET Core minimal API — the DEFAULT template for a new .NET web service
// since .NET 6. There is no namespace, no class and no method: every line here
// is a `global_statement`, and the route registrations are plain invocation
// expressions hanging off `app`. Attribute routing never appears, so a parser
// that only reads `[HttpGet]` sees an empty server.
var builder = WebApplication.CreateBuilder(args);
var app = builder.Build();

app.MapGet("/health", () => "ok");

app.MapPost("/orders", (Order o) => Results.Ok(o));

// Route parameter in the template — must survive verbatim into the ROUTE qname
// so `normalise_http_path` can pair it with a client's interpolated segment.
app.MapGet("/orders/{id}", (int id) => Results.Ok(id));

app.Run();

record Order(int Id, string Sku);
