import 'package:flutter_riverpod/flutter_riverpod.dart';

class TokenStorage {
  Future<void> write(String token) async {}
  Future<void> clear() async {}

  /// Read by nothing: stays a dead_symbol row.
  Future<String?> readRefresh() async => null;
}

final tokenStorageProvider = Provider.autoDispose<TokenStorage>((ref) => TokenStorage());
