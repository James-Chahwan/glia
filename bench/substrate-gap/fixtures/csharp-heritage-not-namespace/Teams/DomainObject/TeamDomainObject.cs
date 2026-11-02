using Shop.Core.Commands;

namespace Shop.Teams.DomainObject;

public class TeamEntity
{
    public string Name { get; set; }
}

public class Team : TeamEntity
{
    public int Members { get; set; }
}

public sealed class TeamDomainObject : DomainObject<Team>
{
    public override void Apply(Team state)
    {
        Snapshot = state;
    }
}
