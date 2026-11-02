using StoreBase = Shop.Core.Storage.Store;

namespace Shop.Domain.Contents;

public sealed class SnapshotStore : StoreBase
{
    public override void Save(string key) { }
}
