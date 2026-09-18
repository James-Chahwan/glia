// LB.7d control: a C# 11 `file` type is file-local by the language, so it
// keeps the file segment (Svc::Helpers::Scratch) and never merges with the
// `file class Scratch` of Other.cs.
namespace Shop.Core;

file class Scratch
{
    public int A()
    {
        return 1;
    }
}
