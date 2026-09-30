import 'package:redis/redis.dart';

Future<void> listen(Command cmd) async {
  final pubsub = PubSub(cmd);
  pubsub.subscribe(['orders']);
  pubsub.getStream().listen((msg) => print(msg));
}
