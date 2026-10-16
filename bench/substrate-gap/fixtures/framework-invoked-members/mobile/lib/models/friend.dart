class Friend {
  Friend(this.id, this.name);
  final String id;
  final String name;

  factory Friend.fromJson(Map<String, dynamic> j) => Friend(j['id'] as String, j['name'] as String);
}

class Orphan {
  Orphan(this.id);
  final String id;
}
