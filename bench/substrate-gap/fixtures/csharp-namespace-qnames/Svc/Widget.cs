// LB.7d: no namespace. The type is directory-scoped (Svc::Widget), never
// file-scoped (HEAD: Svc::Widget::Widget).
public class Widget
{
    public int Run()
    {
        return Helper();
    }

    int Helper()
    {
        return 1;
    }
}
