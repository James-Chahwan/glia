import 'package:kafka_dart/kafka_dart.dart';

Future<void> consume() async {
  final consumer = await KafkaFactory.createAndInitializeConsumer(
    bootstrapServers: 'localhost:9092',
    groupId: 'shop',
  );
  await consumer.subscribe(['orders']);
  final message = await consumer.pollMessage();
  print(message?.payload.value);
}
