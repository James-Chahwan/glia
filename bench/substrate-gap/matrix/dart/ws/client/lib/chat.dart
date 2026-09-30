import 'package:web_socket_channel/web_socket_channel.dart';

WebSocketChannel connect() {
  final channel = WebSocketChannel.connect(Uri.parse('ws://api/ws/chat'));
  channel.stream.listen((m) => print(m));
  return channel;
}
