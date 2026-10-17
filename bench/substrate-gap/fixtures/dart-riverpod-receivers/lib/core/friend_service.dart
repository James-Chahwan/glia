import 'package:flutter_riverpod/flutter_riverpod.dart';

class FriendService {
  FriendService(this._ref);
  final Ref _ref;

  Future<List<String>> listAll() async => const [];

  Future<void> accept(String id) async {}
}

final friendServiceProvider = Provider<FriendService>(FriendService.new);

final friendsProvider = FutureProvider<List<String>>((ref) async {
  return ref.read(friendServiceProvider).listAll();
});
