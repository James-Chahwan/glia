import 'package:googleapis/pubsub/v1.dart';

Future<void> pull(PubsubApi api) async {
  await api.projects.subscriptions.pull(PullRequest(maxMessages: 10), 'projects/shop/subscriptions/orders');
}
