import 'package:dio/dio.dart';
import 'package:injectable/injectable.dart';

@lazySingleton
class AuthService {
  final Dio dio;
  AuthService(this.dio);

  Future<void> login() async {
    await dio.post('/login');
  }
}
