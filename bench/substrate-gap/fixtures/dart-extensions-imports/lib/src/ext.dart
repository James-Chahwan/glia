import 'money.dart' as m show Money, round2;

extension on m.Money {
  m.Money doubled() => m.Money(m.round2(cents * 2));
}

extension on String {
  int toCents() => m.round2(length);
}

int total() => m.round2(1);
