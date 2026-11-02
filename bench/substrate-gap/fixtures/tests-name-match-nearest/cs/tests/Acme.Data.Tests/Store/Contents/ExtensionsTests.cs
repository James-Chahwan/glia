using Xunit;
using Sut = Acme.Ops.Extensions;

namespace Acme.Data.Tests.Store.Contents
{
    public class ExtensionsTests
    {
        [Fact]
        public void Retry_adds_one()
        {
            Assert.Equal(2, Sut.Retry(1));
        }
    }
}
