import 'package:dio/dio.dart';
import 'package:shelf/shelf.dart';
import 'package:shelf_router/shelf_router.dart';

class AuthApi {
  AuthApi(this.conn);
  final Dio conn;

  Dio get session => conn;

  Future<String?> refresh() async {
    final fresh = Dio(BaseOptions(baseUrl: 'https://api.example.net'))
      ..interceptors.add(LogInterceptor());
    final res = await fresh.post(
      '/auth/refresh',
      options: Options(extra: {'skipAuth': true}),
    );
    return res.data['accessToken'] as String?;
  }

  Future<void> me() async {
    Dio transport = Dio();
    await transport.get('/auth/me');
  }

  Future<void> changePassword(String pw) async {
    await conn.put('/auth/password', data: {'password': pw});
  }

  Future<void> logout() async {
    await session.delete('/auth/session');
  }
}

Response _health(Request req) => Response.ok('ok');

Router buildRouter() {
  final router = Router();
  router.get('/health', _health);
  return router;
}
