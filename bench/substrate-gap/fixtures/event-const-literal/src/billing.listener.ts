import { Injectable } from "@nestjs/common";
import { OnEvent } from "@nestjs/event-emitter";
import { OrderEvents } from "./orders.events";

@Injectable()
export class BillingListener {
  @OnEvent(OrderEvents.Paid)
  onPaid(payload: { id: string }) {}
}
