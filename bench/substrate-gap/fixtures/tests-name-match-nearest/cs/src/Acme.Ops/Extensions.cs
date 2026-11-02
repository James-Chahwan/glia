namespace Acme.Ops
{
    public static class Extensions
    {
        public static int Retry(this int attempts)
        {
            return attempts + 1;
        }
    }
}
