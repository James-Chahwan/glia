import { Injectable } from "@nestjs/common";
import { EventEmitter2 } from "@nestjs/event-emitter";
import { OrderEvents } from "./orders.events";

@Injectable()
export class OrdersService {
  constructor(private eventEmitter: EventEmitter2) {}

  create(id: string) {
    this.eventEmitter.emit(OrderEvents.Created, { id });
  }
}
