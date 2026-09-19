#include "Widget.h"

class Local {
 public:
  int a();
  int b() { return 4; }
};

int Local::a() {
  return this->b();
}

void Widget::run() {
  this->helper();
}
