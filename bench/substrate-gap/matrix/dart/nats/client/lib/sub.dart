import 'package:dart_nats/dart_nats.dart';

void listen(Client client) {
  final sub = client.sub('orders');
  sub.stream.listen((msg) => print(msg.string));
}
