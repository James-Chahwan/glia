// NestJS REST controller: @Query() here is the @nestjs/common query-string
// parameter decorator, not GraphQL.
import { Controller, Get, Query } from "@nestjs/common";

@Controller("users")
export class UsersController {
  @Get()
  async list(
    @Query() filter: ListUsersDto,
  ) {
    return filter;
  }
}
