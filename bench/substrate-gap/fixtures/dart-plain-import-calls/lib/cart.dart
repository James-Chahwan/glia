import 'package:shop/src/util.dart';
import 'src/fmt.dart';

class Cart {
  String total(int c) {
    final m = Money.zero();
    shout('x');
    return formatPrice(c);
  }
}

void main() {
  print(formatPrice(1));
  Money();
}
