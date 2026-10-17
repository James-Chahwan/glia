class Env {
  static const bool useLocal = false;
  static const String clientId = 'app-client';

  static String get baseUrl => useLocal ? 'http://localhost:8080' : 'https://api.example.com';
  static int get port => useLocal ? 8080 : 443;

  /// Read by nothing: stays a dead_symbol row.
  static bool get debugBanner => false;
}
