import 'package:injectable/injectable.dart';

import 'api_service.dart';

@injectable
class HomeController {
  final ApiService api;

  HomeController(this.api);
}
