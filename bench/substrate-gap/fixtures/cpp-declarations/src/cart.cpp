#include "cart.hpp"

namespace {
int clamp(int v) { return v < 0 ? 0 : v; }
}

namespace shop {
int Cart::add(int qty) {
    return clamp(qty);
}

int Cart::Line::total() const { return qty; }
}

extern "C" int legacy_entry(int v) {
    return clamp(v);
}
