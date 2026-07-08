import { Resolver, Query, Args } from '@nestjs/graphql';
import { UserService } from './user.service';

@Resolver('User')
export class UserResolver {
  constructor(private readonly userService: UserService) {}

  @Query(() => User)
  async getUser(@Args('id') id: string) {
    return this.userService.findOne(id);
  }
}
