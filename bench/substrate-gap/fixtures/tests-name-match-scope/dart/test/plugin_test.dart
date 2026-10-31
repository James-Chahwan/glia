import 'package:flutter_test/flutter_test.dart';
import 'package:plugin_web/plugin.dart';

void main() {
  test('platformName', () {
    expect(PluginWeb().platformName(), 'web');
  });
}
