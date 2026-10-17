class Friend {
  Friend(this.id);
  final String id;

  factory Friend.fromJson(Map<String, dynamic> j) => Friend(j['id'] as String);
}

class Interest {
  Interest(this.name);
  final String name;

  /// A same-named factory no tear-off names: stays a dead_symbol row.
  factory Interest.fromJson(Map<String, dynamic> j) => Interest(j['name'] as String);

  /// Read by nothing (an int loop variable's `isEven` is not this one):
  /// stays a dead_symbol row.
  bool get isEven => name.length.isEven;

  Map<String, dynamic> toJson() => {'name': name};
}
