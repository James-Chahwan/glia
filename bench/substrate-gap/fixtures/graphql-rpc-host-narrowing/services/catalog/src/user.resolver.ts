import { Resolver, Query } from "@nestjs/graphql";

@Resolver()
export class CatalogUserResolver {
  @Query(() => String)
  getUser() {
    return "c";
  }
}
