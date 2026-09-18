// NestJS: @OnEvent / @EventPattern name real events; the control.
import { Injectable } from "@nestjs/common";
import { OnEvent } from "@nestjs/event-emitter";
import { EventPattern } from "@nestjs/microservices";

@Injectable()
export class OrdersListener {
  @OnEvent("order.shipped")
  onShipped(payload: unknown) {}

  @EventPattern("order.refunded")
  onRefunded(payload: unknown) {}
}
