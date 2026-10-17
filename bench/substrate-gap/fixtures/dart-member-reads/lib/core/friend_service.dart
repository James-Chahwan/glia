import 'env.dart';
import 'models.dart';

class FriendService {
  bool get isReady => Env.baseUrl.isNotEmpty;

  List<Friend> listAll(List<Map<String, dynamic>> raw) => raw.map(Friend.fromJson).toList();
}
