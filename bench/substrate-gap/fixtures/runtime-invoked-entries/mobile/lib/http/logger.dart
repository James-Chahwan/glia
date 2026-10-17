import 'package:dio/dio.dart';

/// Imports dio, but is no Interceptor: its onRequest is only a name.
class Logger {
  void onRequest(RequestOptions options) {}
}
