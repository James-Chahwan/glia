namespace Acme.Tests.Tools
{
    public static class Extensions
    {
        public static string Fixture(this string name)
        {
            return "fixture:" + name;
        }
    }
}
