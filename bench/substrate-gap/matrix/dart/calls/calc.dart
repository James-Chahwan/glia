class Calc {
  int add(int a, int b) => a + b;

  int total(List<int> xs) {
    var acc = 0;
    for (final x in xs) {
      acc = add(acc, x);
    }
    return acc;
  }
}
