import 'package:grpc/grpc.dart';
import 'src/generated/orders.pbgrpc.dart';

Future<void> getOrder() async {
  final channel = ClientChannel('localhost', port: 50051, options: const ChannelOptions(credentials: ChannelCredentials.insecure()));
  final stub = OrderServiceClient(channel);
  await stub.getOrder(OrderRequest()..id = '1');
}
