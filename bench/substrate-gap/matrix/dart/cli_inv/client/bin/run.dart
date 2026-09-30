import 'dart:io';

Future<void> main() async {
  await Process.run('mytool', ['sync']);
}
