// LB.14: a C# 11 `file` type is visible only in its own file, so the
// `file class Scratch` of this block namespace takes the file scope
// (Svc::Helpers::Scratch) and never merges with the one in Other.cs.
namespace Shop.Core
{
    file class Scratch
    {
        public int A()
        {
            return Inner();
        }

        int Inner()
        {
            return 1;
        }
    }

    // Control: a non-`file` type keeps the namespace scope (Shop::Core::Helpers).
    public class Helpers
    {
        public int Use()
        {
            return 0;
        }
    }
}
