import { Injectable } from "@nestjs/common";
import { OnEvent } from "@nestjs/event-emitter";

@Injectable()
export class AuditListener {
  @OnEvent("order.created")
  onCreated(payload: { id: string }) {}
}
