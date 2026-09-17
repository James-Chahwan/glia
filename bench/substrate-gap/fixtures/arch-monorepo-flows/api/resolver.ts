// GraphQL server: the getUser resolver web/query.ts calls.
import { Resolver, Query, Args } from '@nestjs/graphql';

@Resolver('User')
export class UserResolver {
  @Query(() => User)
  async getUser(@Args('id') id: string) {
    return { id, name: 'ada' };
  }
}
