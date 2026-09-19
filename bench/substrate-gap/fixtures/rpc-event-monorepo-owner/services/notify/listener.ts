// A NestJS microservice: @EventPattern listens on the transport, not on an in-process bus.
import { Controller } from "@nestjs/common";
import { EventPattern } from "@nestjs/microservices";

@Controller()
export class ShipmentListener {
  @EventPattern("orderShipped")
  onShipped(data: unknown) {
    console.log(data);
  }
}
