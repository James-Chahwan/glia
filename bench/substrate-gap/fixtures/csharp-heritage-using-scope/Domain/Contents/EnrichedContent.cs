using Shop.Core.Model;

namespace Shop.Domain.Contents;

public record EnrichedContent : Content
{
    public string SchemaName { get; init; } = "";
}
