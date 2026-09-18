// LB.7d control: the block namespace form never doubled and is unchanged,
// including a `file` type inside it (Shop::Legacy::X).
namespace Shop.Legacy
{
    public class Legacy
    {
        public void Go()
        {
        }
    }

    file class X
    {
        public void Y()
        {
        }
    }
}
