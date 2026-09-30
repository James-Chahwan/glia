import { Injectable } from "@nestjs/common";
import { OnEvent } from "@nestjs/event-emitter";

@Injectable()
export class BillingListener {
  @OnEvent("order.placed")
  charge(payload: { id: string }) {}
}
