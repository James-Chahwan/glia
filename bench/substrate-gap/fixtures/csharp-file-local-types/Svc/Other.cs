// LB.14: the second file-local `Scratch` of block namespace Shop.Core, with its
// own `Inner`, and a file-local struct: both take the file scope (Svc::Other).
namespace Shop.Core
{
    file class Scratch
    {
        public int B()
        {
            return Inner();
        }

        int Inner()
        {
            return 2;
        }
    }

    file struct Pair
    {
        public int Sum()
        {
            return 0;
        }
    }
}
