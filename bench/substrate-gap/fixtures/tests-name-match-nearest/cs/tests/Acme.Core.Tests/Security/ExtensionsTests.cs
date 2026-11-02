using Acme.Core.Security;
using Xunit;

namespace Acme.Core.Tests.Security
{
    public class ExtensionsTests
    {
        [Fact]
        public void Admin_role_is_admin()
        {
            Assert.True("admin".IsAdmin());
        }
    }
}
