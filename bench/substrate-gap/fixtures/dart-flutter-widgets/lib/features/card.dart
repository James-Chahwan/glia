import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

class CardPage extends ConsumerStatefulWidget {
  const CardPage({super.key, required this.onTap});
  final VoidCallback onTap;

  @override
  ConsumerState<CardPage> createState() => _CardPageState();
}

class _CardPageState extends ConsumerState<CardPage> {
  int taps = 0;

  @override
  void initState() {
    super.initState();
  }

  @override
  void dispose() {
    super.dispose();
  }

  void _save() {
    setState(() => taps++);
  }

  void onTap() {}

  @override
  Widget build(BuildContext context) {
    final onTap = widget.onTap;
    return Column(children: [
      const _Dot(),
      new _Label('taps'),
      ElevatedButton(onPressed: _save, child: Text('$taps')),
      GestureDetector(onTap: onTap, child: const SizedBox()),
    ]);
  }
}

class _Dot extends StatelessWidget {
  const _Dot();

  @override
  Widget build(BuildContext context) => const SizedBox(width: 8);
}

class _Label extends StatelessWidget {
  const _Label(this.text);
  final String text;

  @override
  Widget build(BuildContext context) => Text(text);
}

class CartModel extends ChangeNotifier {
  void clear() {}

  @override
  void dispose() {
    super.dispose();
  }
}
