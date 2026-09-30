import { Injectable } from "@nestjs/common";
import { OnEvent } from "@nestjs/event-emitter";

@Injectable()
export class AuditListener {
  @OnEvent("order.placed")
  record(payload: { id: string }) {}
}
