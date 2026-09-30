import 'package:event_bus/event_bus.dart';

class OrderPlaced {
  final String id;
  OrderPlaced(this.id);
}

final eventBus = EventBus();

void wire() {
  eventBus.on<OrderPlaced>().listen((e) => print(e.id));
  eventBus.fire(OrderPlaced('o-1'));
}
