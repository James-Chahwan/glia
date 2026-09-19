import 'package:dio/dio.dart';

final client = Dio();

mixin Greets {
  String greet() => hello();
  String hello() => 'hello';
}

extension Shout on String {
  String shout() => twice();
  String twice() => this + this;
}

enum Color {
  red,
  green;

  String label() => describe();
  String describe() => name;
}

class Api {
  void warm() {}

  Api() {
    client.get('/boot');
  }

  Future<Response> get status => client.get('/status');

  int compute() => 1;

  int get area => compute();

  set area(int v) => store(v);

  void store(int v) {}
}

extension on Api {
  int doubled() => area * 2;
}

extension on String {
  String whisper() => toLowerCase();
}

extension type Meters(int value) {
  int plus(int o) => value + o;
  int twicePlus(int o) => plus(plus(o));
}
