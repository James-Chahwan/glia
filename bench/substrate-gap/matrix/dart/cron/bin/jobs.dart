import 'package:cron/cron.dart';

void main() {
  final cron = Cron();
  cron.schedule(Schedule.parse('*/5 * * * *'), () async {
    print('tick');
  });
}
