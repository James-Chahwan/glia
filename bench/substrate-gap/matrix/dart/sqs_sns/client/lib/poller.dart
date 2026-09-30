import 'package:aws_sqs_api/sqs-2012-11-05.dart';

Future<void> poll(SQS sqs) async {
  await sqs.receiveMessage(queueUrl: 'https://sqs.us-east-1.amazonaws.com/123/orders');
}
