using Shop.Core.Commands;

namespace Shop.Domain.Teams;

public sealed class TeamState
{
    public string Name { get; set; } = "";
}

public sealed class TeamDomainObject : DomainObject<TeamState>
{
}
