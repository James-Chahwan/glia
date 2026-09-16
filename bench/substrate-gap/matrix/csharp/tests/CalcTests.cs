using Xunit;

namespace Shop.Tests;

public class CalcTests
{
    [Fact]
    public void Add_ReturnsSum()
    {
        Assert.Equal(5, new Calc().Add(2, 3));
    }
}
