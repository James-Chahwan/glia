import 'package:dio/dio.dart';

class FriendService {
  FriendService(this.dio);
  final Dio dio;

  Future<void> listAll() async {
    final res = await dio.get<dynamic>('/protected/friends');
  }

  Future<void> accept(String publicId) async {
    await dio.post('/protected/friends/accept/$publicId');
  }
}
