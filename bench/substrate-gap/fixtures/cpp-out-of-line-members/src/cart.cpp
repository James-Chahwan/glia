#include "../include/shop/cart.hpp"

namespace shop {

int Cart::total() {
  return this->tax();
}

}  // namespace shop

void shop::init() {}
