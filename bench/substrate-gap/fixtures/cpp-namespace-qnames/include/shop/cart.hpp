#pragma once

namespace shop {

class Cart {
 public:
  int total();
  int tax() { return 1; }
};

}  // namespace shop
