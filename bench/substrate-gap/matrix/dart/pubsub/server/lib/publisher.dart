import 'package:googleapis/pubsub/v1.dart';

Future<void> publish(PubsubApi api, String data) async {
  await api.projects.topics.publish(PublishRequest(messages: [PubsubMessage(data: data)]), 'projects/shop/topics/orders');
}
