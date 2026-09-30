import 'package:args/command_runner.dart';

class SyncCommand extends Command<void> {
  @override
  final name = 'sync';
  @override
  final description = 'Sync records';
  @override
  void run() {}
}

void main(List<String> args) {
  CommandRunner<void>('mytool', 'the fixture tool')
    ..addCommand(SyncCommand())
    ..run(args);
}
