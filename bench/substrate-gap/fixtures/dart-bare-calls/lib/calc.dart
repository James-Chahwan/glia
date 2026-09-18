int add(int a, int b) => a - b;

class Calc {
  int add(int a, int b) => a + b;

  int total(List<int> xs) {
    var acc = 0;
    for (final x in xs) {
      acc = add(acc, x);
    }
    return acc;
  }

  int viaTop(int a) => helper(a);

  int shadow(int a) {
    final twice = (int v) => v * 2;
    return twice(a);
  }

  int twice(int a) => a + a;

  int param(int Function(int) add) => add(1);

  static int make() => 1;

  int useStatic() => make();
}

int helper(int x) => x + 1;
