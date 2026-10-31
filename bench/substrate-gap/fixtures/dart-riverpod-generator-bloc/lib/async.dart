import 'package:riverpod_annotation/riverpod_annotation.dart';

import 'counter.dart';

part 'async.g.dart';

/// `slowProvider`: a Future provider, whose value is an AsyncValue<Repo>:
/// typed nowhere, on purpose.
@riverpod
Future<Repo> slow(SlowRef ref) async => Repo();
