namespace Shop.Core.Commands;

public abstract record Entity
{
    public long Version { get; init; }
}
