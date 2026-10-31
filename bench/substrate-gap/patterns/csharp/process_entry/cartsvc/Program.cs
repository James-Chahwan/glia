using System;
using System.Threading;

namespace Cart
{
    public static class Program
    {
        public static void Main(string[] args)
        {
            Console.WriteLine("cart service started");
            Thread.Sleep(Timeout.Infinite);
        }
    }
}
