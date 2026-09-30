import 'package:mqtt_client/mqtt_client.dart';
import 'package:mqtt_client/mqtt_server_client.dart';

void report(MqttServerClient client) {
  final builder = MqttClientPayloadBuilder()..addString('21');
  client.publishMessage('sensors/temp', MqttQos.atLeastOnce, builder.payload!);
}
