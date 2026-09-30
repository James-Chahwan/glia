#include "cart.hpp"
#include "legacy.hpp"

namespace shop {
int Cart::add(int qty) {
    reset();
    return qty + Cart::count() + box.get() + codec_encode("a");
}
int Cart::count() { return 0; }
}

using namespace shop;

int run() {
    Cart c;
    return c.add(1);
}
