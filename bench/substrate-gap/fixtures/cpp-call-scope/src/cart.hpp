#pragma once
#include "codec.h"
namespace shop {
class Box {
public:
    int get() const { return 1; }
};

class Cart {
public:
    int add(int qty);
    static int count();
    void clear() { reset(); }
    void reset() {}
private:
    Box box;
};
}
