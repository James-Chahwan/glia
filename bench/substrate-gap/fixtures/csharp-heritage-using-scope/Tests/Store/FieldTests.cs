using Xunit;
using Shop.Core;

namespace Shop.Tests.Store;

public class FieldTests
{
    private class Entity
    {
        public string Id { get; set; } = "";
    }

    private sealed class Inherited : Entity
    {
    }
}
