int round2(int v) => v;

class Money {
  final int cents;
  Money(this.cents) {
    validate();
  }
  Money.zero() : cents = round2(0);
  factory Money.parse(String s) {
    return Money(int.parse(s));
  }
  Money operator +(Money other) => Money(round2(cents + other.cents));
  void validate() {}
}

abstract class Repo {
  Money load();
  void save(Money m);
}

enum Currency { aud, usd }

Money total() => Money.zero() + Money.parse('1');
