// NestJS resolver: two decorator nouns (Resolver, Query) and one real field.
import { Resolver, Query } from "@nestjs/graphql";

@Resolver()
export class UsersResolver {
  @Query()
  async getUsers() {
    return [];
  }
}
