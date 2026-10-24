import 'package:event_bus/event_bus.dart';

class OrderPlaced {
  final String id;
  OrderPlaced(this.id);
}

class GhostEvent {}

final eventBus = EventBus();

void wire() {
  // eventBus.fire(GhostEvent());
  final doc = 'eventBus.fire(GhostEvent())';
  eventBus.fire(OrderPlaced('o-1'));
}
