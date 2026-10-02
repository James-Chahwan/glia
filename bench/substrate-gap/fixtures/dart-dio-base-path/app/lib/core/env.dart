class Env {
  static const bool useLocalApi = bool.fromEnvironment('USE_LOCAL_API');
  static const String prodApiHost = 'api.example.net';

  static String get apiBaseUrl {
    final scheme = useLocalApi ? 'http' : 'https';
    return '$scheme://$prodApiHost/api';
  }
}
