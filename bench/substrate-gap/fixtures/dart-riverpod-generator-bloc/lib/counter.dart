import 'package:riverpod_annotation/riverpod_annotation.dart';

part 'counter.g.dart';

/// riverpod_generator writes `counterProvider` into counter.g.dart: its
/// notifier is a Counter, its value an int.
@riverpod
class Counter extends _$Counter {
  @override
  int build() => 0;

  void inc() {
    state = state + 1;
  }

  void reset() {
    state = 0;
  }
}

/// `repoProvider`: its value is a Repo.
@riverpod
Repo repo(RepoRef ref) => Repo();

/// `todosProvider`: an AsyncNotifier, whose value is an AsyncValue (untyped)
/// and whose notifier is a Todos.
@Riverpod(keepAlive: true)
class Todos extends _$Todos {
  @override
  Future<List<String>> build() async => [];

  Future<void> add(String title) async {}
}

class Repo {
  void load() {}
}
