int round2(int v) => v;
int seed() => 3;

final int defaultTax = seed();
var cache = <String, int>{}, hits = round2(1);

class Money {
  final int cents;
  const Money(this.cents);
}
