part 'user.g.dart';

class User {
  final String name;

  User(this.name);

  Map<String, dynamic> toJson() => userToJson(this);
}
