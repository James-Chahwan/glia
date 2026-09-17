import 'package:dio/dio.dart';

/// A3.3: a Flutter client calling its API by ABSOLUTE URL. Before A3.3 the
/// Dart parser dropped any first argument not starting with `/`, so this call
/// produced no ENDPOINT node at all.
class ApiClient {
  final Dio dio = Dio();

  Future<void> createUser(Map body) async {
    await dio.post('https://api.example.com/users', data: body);
  }
}
