import 'package:mqtt_client/mqtt_client.dart';
import 'package:mqtt_client/mqtt_server_client.dart';

void listen(MqttServerClient client) {
  client.subscribe('sensors/temp', MqttQos.atMostOnce);
}
