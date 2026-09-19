// NestJS code-first GraphQL resolver: the control.
import { Args, Mutation, Query, Resolver } from "@nestjs/graphql";

@Resolver(() => Recipe)
export class RecipesResolver {
  @Query(() => [Recipe])
  async recipes() {
    return [];
  }

  @Mutation(() => Recipe)
  async addRecipe(@Args("title") title: string) {
    return { title };
  }
}
