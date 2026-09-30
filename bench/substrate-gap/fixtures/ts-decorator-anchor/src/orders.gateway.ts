import { Injectable } from "@nestjs/common";
import { OnEvent } from "@nestjs/event-emitter";
import { Resolver, Query } from "@nestjs/graphql";

@Resolver()
@Injectable()
export class OrdersGateway {
  /** Records a shipped order. */
  @OnEvent("order.shipped")
  onShipped(payload: { id: string }) {
    return payload.id;
  }

  @Query(() => String)
  order() {
    return "x";
  }
}
