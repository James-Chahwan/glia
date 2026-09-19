import { Resolver, Query, Args } from '@nestjs/graphql';

@Resolver('User')
export class MeResolver {
  @Query(() => String)
  async getUser(@Args('id') id: string) {
    return id;
  }
}
