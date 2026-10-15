#include <app/box.h>
int Box::get() const { return 1; }
void Box::put(int v) {}
void Box::clear() {}
Box make_box() { return Box(); }
Box* find_box(int id) { return nullptr; }
