import 'user_repo.dart';

class UserService {
  final UserRepo repo;
  final _other = UserRepo();

  UserService(this.repo);

  String get(int id) {
    return repo.find(id);
  }

  String other(int id) {
    return _other.find(id);
  }

  String viaThis(int id) {
    return this.repo.find(id);
  }
}
