import 'dart:io';

String databaseUrl() => Platform.environment['DATABASE_URL'] ?? '';
