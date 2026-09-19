#include "point.h"
#include <stdlib.h>

struct point *point_new(int x, int y) {
  struct point *p = malloc(sizeof(struct point));
  p->x = x;
  p->y = y;
  return p;
}
