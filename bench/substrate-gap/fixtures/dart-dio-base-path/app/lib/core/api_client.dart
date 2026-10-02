import 'package:dio/dio.dart';

import 'env.dart';

Dio buildDio() {
  return Dio(BaseOptions(baseUrl: Env.apiBaseUrl, connectTimeout: const Duration(seconds: 15)));
}

Dio retryDio(RequestOptions retry) => Dio(BaseOptions(baseUrl: retry.baseUrl));
