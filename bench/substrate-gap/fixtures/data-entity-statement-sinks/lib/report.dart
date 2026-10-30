import 'package:sqflite/sqflite.dart';

Future<List<Map<String, Object?>>> report(Database db) async {
  final sql = StringBuffer();
  sql.write("SELECT id FROM refunds");
  return db.rawQuery(sql.toString());
}
