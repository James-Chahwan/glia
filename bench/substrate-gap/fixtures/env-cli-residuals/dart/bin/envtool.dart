import 'package:args/command_runner.dart';

class SyncCommand extends Command<void> {
  @override
  final name = 'sync';
  @override
  final description = 'Sync the env.';
  @override
  List<String> get aliases => const ['pull-env'];

  @override
  void run() {}
}

void main(List<String> args) =>
    CommandRunner<void>('envtool', 'Env tool.')..addCommand(SyncCommand())..run(args);
