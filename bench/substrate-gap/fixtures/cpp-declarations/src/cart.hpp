#pragma once
namespace shop {
#include "detail.hpp"

template <typename T>
class Box {
public:
    T get() const { return value; }
    T value;
};

union Number {
    int i;
    float f;
};

class Cart {
public:
    struct Line {
        int qty;
        int total() const { return qty * 2; }
    };
    int add(int qty);
};
}
