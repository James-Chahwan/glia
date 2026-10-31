import 'package:app/app.dart';

void main() {
  Svc().go();
  Svc.open();
}

class Runner {
  final Svc svc = Svc();

  void run() {
    svc.go();
  }
}
