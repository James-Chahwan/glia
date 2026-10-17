import 'package:flutter/material.dart';

import '../core/env.dart';
import '../core/friend_service.dart';
import '../core/models.dart';

class FriendsPage extends StatefulWidget {
  const FriendsPage({super.key, this.onTap});
  final VoidCallback? onTap;

  @override
  State<FriendsPage> createState() => _FriendsPageState();
}

class _FriendsPageState extends State<FriendsPage> {
  static const _tabs = [_Tab('home'), _Tab('friends')];

  final FriendService _friends = FriendService();
  final List<Interest> _interests = [];
  bool _busy = false;
  int _modeValue = 0;

  bool get _canSubmit => !_busy && _friends.isReady;

  String get _title => 'Friends';

  set _mode(int v) => _modeValue = v;

  void _submit() {
    if (!_canSubmit) return;
    _friends.listAll(const []);
  }

  void _google() {
    _mode = 1;
  }

  void _cancel() {}

  void _make() {}

  /// Named like the `onPressed:` labels below, referenced by none of them:
  /// stays a dead_symbol row.
  void onPressed() {}

  void _register(FriendService Function() make) {}

  /// The loop's `i` receives `.isEven`, so the closure's `i` is typed
  /// nowhere in this method.
  void _dump() {
    for (final i in [1, 2]) {
      if (i.isEven) {}
    }
    _interests.map((i) => i.toJson()).toList();
  }

  /// A local `_canSubmit` shadows the getter, and an untyped local
  /// `_friends` the field declared FriendService: neither read is typed
  /// through the member it shadows. A cascade's `..onPressed` names a member
  /// of the local's object, not this class's `onPressed`.
  void _shadow() {
    final _canSubmit = false;
    if (_canSubmit) {}
    final dynamic _friends = widget.onTap;
    if (_friends.isReady == true) {}
    _friends..onPressed = null;
  }

  @override
  Widget build(BuildContext context) {
    _register(FriendService.new);
    final payload = _interests.map((i) => i.toJson()).toList();
    return Column(children: [
      Text('${Env.port} $payload $_title $_modeValue'),
      Text(Env.clientId),
      Text(_tabs.first.label),
      ElevatedButton(onPressed: _busy ? null : _submit, child: const Text('Add')),
      ElevatedButton(
        onPressed: this._canSubmit ? (_busy ? _cancel : _google) : null,
        child: const Text('Google'),
      ),
      TextButton(onPressed: widget.onTap ?? _make, child: const Text('More')),
      TextButton(
        onPressed: () {
          _dump();
          _shadow();
        },
        child: const Text('Dump'),
      ),
    ]);
  }
}

class _Tab {
  const _Tab(this.label);
  final String label;
}
