import 'package:dio/dio.dart';
import 'package:go_router/go_router.dart';

/// go_router navigation table. `GoRoute` is BROWSER/app navigation — the
/// parser emits it with the method string "ANY" (there is no verb to record).
final router = GoRouter(
  routes: [
    GoRoute(path: '/users', builder: buildUsersPage),
  ],
);

Object buildUsersPage(Object a, Object b) => a;

/// The same app also calls a REMOTE `/users` over HTTP.
class ApiClient {
  final Dio dio;
  ApiClient(this.dio);

  Future<Object> fetchUsers() async {
    final res = await dio.get('/users');
    return res.data as Object;
  }
}
