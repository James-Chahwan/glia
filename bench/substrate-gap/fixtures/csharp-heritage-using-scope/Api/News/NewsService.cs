using Microsoft.Extensions.Options;
using Shop.Flows.Utils;
using Vendor.ClientLibrary;

namespace Shop.Api.News;

public sealed class NewsDto
{
    public string Title { get; set; } = "";
}

public sealed class NewsEntity : Content<NewsDto>
{
}
