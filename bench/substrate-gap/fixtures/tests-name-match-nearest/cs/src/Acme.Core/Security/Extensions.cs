namespace Acme.Core.Security
{
    public static class Extensions
    {
        public static bool IsAdmin(this string role)
        {
            return role == "admin";
        }
    }
}
