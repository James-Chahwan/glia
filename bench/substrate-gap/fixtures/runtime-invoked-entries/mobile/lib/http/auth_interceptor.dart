import 'package:dio/dio.dart';

/// Dio calls onRequest on every interceptor added to a client.
class AuthInterceptor extends Interceptor {
  AuthInterceptor();

  @override
  void onRequest(RequestOptions options, RequestInterceptorHandler handler) {
    handler.next(options);
  }

  void onRequestLater() {}
}
