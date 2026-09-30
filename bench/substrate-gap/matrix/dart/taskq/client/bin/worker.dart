import 'package:redis_task_queue/redis_task_queue.dart';

Future<void> main() async {
  final worker = await Worker.connect(workerId: 'worker-1', queues: {'default': 1});
  worker.handle('email:welcome', (task, context) async {
    print(task.payload['user_id']);
  });
  await worker.run();
}
