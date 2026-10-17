import 'package:flutter_riverpod/flutter_riverpod.dart';

class GroupService {
  /// FriendService::listAll's look-alike, called by nothing: stays a
  /// dead_symbol row (a provider receiver is typed, never matched by name).
  Future<void> listAll() async {}

  Future<void> create() async {}

  Future<void> leave() async {}
}

final groupServiceProvider = Provider((ref) => GroupService());
