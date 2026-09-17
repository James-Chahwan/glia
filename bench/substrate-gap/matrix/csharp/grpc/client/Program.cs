using Demo;
using Demo.Web;

var builder = WebApplication.CreateBuilder(args);

builder.Services.AddGrpcClient<Greeter.GreeterClient>(options =>
{
    options.Address = new Uri("https://localhost:5001");
});
builder.Services.AddScoped<GreetingService>();

var app = builder.Build();
app.Run();
