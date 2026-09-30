import 'package:sqflite/sqflite.dart';

Future<List<Map<String, Object?>>> allUsers(Database db) {
  return db.query('users');
}

Future<int> addUser(Database db, String email) {
  return db.insert('users', {'email': email});
}
