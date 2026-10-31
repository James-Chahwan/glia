import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'async.dart';
import 'counter.dart';

class Page extends ConsumerWidget {
  const Page({super.key});

  @override
  Widget build(BuildContext context, WidgetRef ref) {
    ref.read(counterProvider.notifier).inc();
    ref.read(repoProvider).load();
    final todos = ref.read(todosProvider.notifier);
    todos.add('milk');
    return const Text('page');
  }

  /// A Future provider's value is an AsyncValue: `.load()` here is no
  /// Repo::load call.
  void poll(WidgetRef ref) {
    ref.watch(slowProvider).load();
  }
}
