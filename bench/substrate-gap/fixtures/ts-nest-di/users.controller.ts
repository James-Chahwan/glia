import { Controller, Get } from "@nestjs/common";
import { UsersService } from "./users.service";

@Controller("users")
export class UsersController {
  // NestJS ctor DI: UsersController INJECTS UsersService; `number` is skipped.
  constructor(private readonly users: UsersService, private readonly pageSize: number) {}

  @Get()
  findAll(): string[] {
    return this.users.findAll();
  }
}
