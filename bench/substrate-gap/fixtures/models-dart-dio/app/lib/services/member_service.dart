import 'package:dio/dio.dart';
import '../models/member.dart';

class MemberService {
  MemberService(this.dio);

  final Dio dio;

  Future<Member> get(String id) async {
    final res = await dio.get<Map<String, dynamic>>('/api/members/$id');
    return Member.fromJson(res.data ?? const {});
  }

  Future<List<Member>> list() async {
    final res = await dio.get<dynamic>('/api/members');
    final raw = res.data as Map<String, dynamic>;
    return (raw['members'] as List)
        .whereType<Map<String, dynamic>>()
        .map(Member.fromJson)
        .toList();
  }

  Future<void> rename(String id, String handle) async {
    await dio.put('/api/members/$id', data: {'handle': handle, 'reason': 'user'});
  }
}
