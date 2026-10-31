import 'package:flutter/material.dart';
import 'base.dart';

/// A screen through the in-repo base: a widget Flutter builds.
class Home extends BaseScreen {
  const Home({super.key});

  @override
  Widget build(BuildContext c) => const Text('x');
}

/// Shares nothing with Flutter: stays dead.
class Plain {
  Widget build(BuildContext c) => const Text('y');
}
