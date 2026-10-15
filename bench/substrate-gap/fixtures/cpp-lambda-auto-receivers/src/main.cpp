#include <app/box.h>

static void helper(int x) {}

int main() {
    auto b = make_box();
    b.get();
    auto p = find_box(1);
    p->put(2);
    auto f = [&](int x) {
        helper(x);
        Box inner;
        inner.clear();
    };
    f(3);
    return 0;
}
