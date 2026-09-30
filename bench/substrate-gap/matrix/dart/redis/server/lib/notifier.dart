import 'package:redis/redis.dart';

Future<void> notify(Command cmd) async {
  await cmd.send_object(['PUBLISH', 'orders', 'order-1']);
}
