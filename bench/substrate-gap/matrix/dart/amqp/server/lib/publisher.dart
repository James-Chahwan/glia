import 'package:dart_amqp/dart_amqp.dart';

Future<void> publish(Client client) async {
  final channel = await client.channel();
  final queue = await channel.queue('orders');
  queue.publish('order-1');
}
