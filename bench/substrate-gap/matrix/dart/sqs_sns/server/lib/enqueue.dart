import 'package:aws_sqs_api/sqs-2012-11-05.dart';

Future<void> enqueue(SQS sqs, String body) async {
  await sqs.sendMessage(queueUrl: 'https://sqs.us-east-1.amazonaws.com/123/orders', messageBody: body);
}
