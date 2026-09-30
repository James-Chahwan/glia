import 'package:launchdarkly_flutter_client_sdk/launchdarkly_flutter_client_sdk.dart';

String variant(LDClient client) {
  return client.boolVariation('new-checkout', false) ? 'new' : 'legacy';
}
