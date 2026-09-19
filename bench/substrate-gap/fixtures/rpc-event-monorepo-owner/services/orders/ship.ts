// The orders service emits orderShipped through a NestJS microservices ClientProxy.
import { ClientProxy } from "@nestjs/microservices";

export class Shipper {
  constructor(private readonly client: ClientProxy) {}

  ship(id: string) {
    this.client.emit("orderShipped", { id });
  }
}
