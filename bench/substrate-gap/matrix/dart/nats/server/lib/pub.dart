import 'package:dart_nats/dart_nats.dart';

void publish(Client client) {
  client.pubString('orders', 'order-1');
}
