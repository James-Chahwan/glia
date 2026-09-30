import { Injectable } from "@nestjs/common";
import { EventEmitter2 } from "@nestjs/event-emitter";
import { OrderEvents } from "./orders.events";

@Injectable()
export class PaymentsService {
  constructor(private eventEmitter: EventEmitter2) {}

  pay(id: string) {
    this.eventEmitter.emit(OrderEvents.Paid, { id });
  }
}
