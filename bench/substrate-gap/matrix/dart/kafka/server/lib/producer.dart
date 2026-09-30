import 'package:kafka_dart/kafka_dart.dart';

Future<void> publish(String body) async {
  final producer = await KafkaFactory.createAndInitializeProducer(
    bootstrapServers: 'localhost:9092',
  );
  await producer.sendMessage(topic: 'orders', payload: body, key: 'k');
  await producer.flush();
}
