import 'package:dio/dio.dart';

final client = Dio();

Future<void> loadUsers() async {
  await client.get('/users');
  helper(1);
}

int helper(int x) => x + 1;

String get banner => describeAll();

String describeAll() => 'all';

int apply(int Function(int) helper) => helper(2);

int commented() // the body follows a comment
    => helper(3);

external void native();

int afterNative() => helper(4);

int get level => helper(5);

void set level(int v) => store(v);

void store(int v) {}
