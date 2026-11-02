namespace Shop.Core.Commands;

public abstract class DomainObject<T>
{
    public T Snapshot { get; protected set; }

    public abstract void Apply(T state);
}
