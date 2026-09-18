// LB.7d: a C# 10 file-scoped namespace is the block form's syntax sugar, so
// the type takes the namespace scope (Shop::Core::Order).
namespace Shop.Core;

public class Order
{
    public int Total()
    {
        return Round();
    }

    int Round()
    {
        return 1;
    }
}
