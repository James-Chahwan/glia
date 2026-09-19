#include "Widget.h"

static int file_helper() { return 2; }

void Widget::run() {
  this->helper();
  file_helper();
}
