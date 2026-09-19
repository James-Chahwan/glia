#ifndef C_POINT_H
#define C_POINT_H

struct point {
  int x;
  int y;
};

struct point *point_new(int x, int y);
int point_format(const struct point *p, char *out, const char *template);

#endif
