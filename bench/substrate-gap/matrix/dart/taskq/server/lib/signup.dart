import 'package:redis_task_queue/redis_task_queue.dart';

Future<void> signup(String userId) async {
  final client = await QueueClient.connect();
  await client.enqueue(Task('email:welcome', {'user_id': userId}), queue: 'default');
}
