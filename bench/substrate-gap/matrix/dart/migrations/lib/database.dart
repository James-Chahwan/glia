import 'package:sqflite/sqflite.dart';

Future<Database> open(String path) {
  return openDatabase(path, version: 1, onCreate: (db, version) async {
    await db.execute('CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT)');
  });
}
