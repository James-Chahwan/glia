import 'package:dart_amqp/dart_amqp.dart';

Future<void> consume(Client client) async {
  final channel = await client.channel();
  final queue = await channel.queue('orders');
  final consumer = await queue.consume();
  consumer.listen((msg) => print(msg.payloadAsString));
}
