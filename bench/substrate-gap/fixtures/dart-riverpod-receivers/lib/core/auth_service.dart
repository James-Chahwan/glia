import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'token_storage.dart';

class AuthState {
  const AuthState({this.user});
  final String? user;

  static const unknown = AuthState();

  bool get isAuthenticated => user != null;

  AuthState copyWith({String? user}) => AuthState(user: user ?? this.user);
}

class AuthService extends StateNotifier<AuthState> {
  AuthService(this._ref) : super(AuthState.unknown);
  final Ref _ref;

  Future<void> login() async {
    state = state.copyWith(user: 'me');
    await _ref.read(tokenStorageProvider).write('token');
  }

  Future<void> logout() async {
    final storage = _ref.read(tokenStorageProvider);
    await storage.clear();
  }
}

final authServiceProvider =
    StateNotifierProvider<AuthService, AuthState>((ref) => AuthService(ref));
