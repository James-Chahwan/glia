import 'package:dio/dio.dart';

/// GAP fixture: the Flutter/Dart client makes an HTTP call the graph is blind
/// to today. `dio.get('/users/:id')` should yield an ENDPOINT node that
/// HttpStackResolver pairs with the Go route -> HTTP_CALLS cross-edge.
class ApiClient {
  final Dio dio;
  ApiClient(this.dio);

  Future<Map<String, dynamic>> fetchUser(String id) async {
    final res = await dio.get('/users/$id');
    return res.data as Map<String, dynamic>;
  }

  Future<void> createUser(Map<String, dynamic> body) async {
    await dio.post('/users', data: body);
  }
}
