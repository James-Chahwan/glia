import 'package:dio/dio.dart';

import 'http/auth_interceptor.dart';
import 'http/logger.dart';

void main() {
  final dio = Dio();
  dio.interceptors.add(AuthInterceptor());
  Logger();
}
