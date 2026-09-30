import { Injectable } from "@nestjs/common";
import { EventEmitter2 } from "@nestjs/event-emitter";

@Injectable()
export class OrdersService {
  constructor(private events: EventEmitter2) {}

  place(id: string) {
    this.events.emit("order.placed", { id });
  }
}
