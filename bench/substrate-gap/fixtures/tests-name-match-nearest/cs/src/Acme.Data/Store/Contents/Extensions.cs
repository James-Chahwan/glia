namespace Acme.Data.Store.Contents
{
    public static class Extensions
    {
        public static string Collection(this string name)
        {
            return "contents_" + name;
        }
    }
}
