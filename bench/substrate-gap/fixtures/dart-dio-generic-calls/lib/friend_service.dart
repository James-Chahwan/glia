import 'package:dio/dio.dart';

class FriendService {
  FriendService(this.dio);
  final Dio dio;

  Future<void> listAll() async {
    final res = await dio.get<dynamic>('/protected/friends');
  }

  Future<void> profile() async {
    final res = await dio.get<Map<String, dynamic>>('/protected/user/profile');
  }

  Future<void> swipe() async {
    await dio.post<void>('/protected/swipe', data: {});
  }

  Future<void> accept(String id) async {
    await dio.post('/protected/friends/accept/$id');
  }
}
