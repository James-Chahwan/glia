import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../core/auth_service.dart';
import '../core/friend_service.dart';
import '../core/group_service.dart';

class LoginPage extends ConsumerStatefulWidget {
  const LoginPage({super.key});

  @override
  ConsumerState<LoginPage> createState() => _LoginPageState();
}

class _LoginPageState extends ConsumerState<LoginPage> {
  Future<void> _submit() async {
    await ref.read(authServiceProvider.notifier).login();
    final svc = ref.read(friendServiceProvider);
    await svc.accept('friend-1');
  }

  void _make() {
    ref.read(groupServiceProvider).create();
  }

  /// `svc` is bound twice here (a provider read, then a closure
  /// parameter), so it is typed nowhere: `svc.leave()` is no
  /// GroupService::leave call.
  void _export(List<dynamic> rooms) {
    final svc = ref.read(groupServiceProvider);
    svc.create();
    rooms.forEach((svc) => svc.leave());
  }

  @override
  Widget build(BuildContext context) {
    final auth = ref.watch(authServiceProvider);
    return Column(children: [
      if (auth.isAuthenticated) const Text('Signed in'),
      ElevatedButton(onPressed: _submit, child: const Text('Sign in')),
      TextButton(onPressed: _make, child: const Text('Groups')),
      TextButton(onPressed: () => _export(const []), child: const Text('Export')),
      TextButton(
        onPressed: () => ref.read(authServiceProvider.notifier).logout(),
        child: const Text('Sign out'),
      ),
    ]);
  }
}
