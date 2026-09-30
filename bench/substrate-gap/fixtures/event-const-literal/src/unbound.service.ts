import { Injectable } from "@nestjs/common";
import { EventEmitter2 } from "@nestjs/event-emitter";
import { Unbound } from "./external";

@Injectable()
export class UnboundService {
  constructor(private eventEmitter: EventEmitter2) {}

  fire() {
    this.eventEmitter.emit(Unbound.Thing, {});
  }
}
