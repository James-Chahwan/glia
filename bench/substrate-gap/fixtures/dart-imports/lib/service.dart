import 'package:collection/collection.dart';
import 'models.dart';

/// service.dart IMPORTS models.dart (relative) and the collection package.
class UserService {
  final List<User> users = [];

  User? firstNamed(String name) =>
      users.firstWhereOrNull((u) => u.name == name);

  void add(User u) => users.add(u);
}
