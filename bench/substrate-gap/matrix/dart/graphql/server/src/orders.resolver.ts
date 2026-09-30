import { Resolver, Query } from "@nestjs/graphql";

@Resolver("Order")
export class OrdersResolver {
  @Query(() => [String])
  async orders() {
    return [];
  }
}
