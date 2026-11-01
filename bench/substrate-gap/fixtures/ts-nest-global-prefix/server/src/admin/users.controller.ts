import { Controller, Get, Post } from '@nestjs/common';

@Controller('users')
export class UsersController {
  @Get('all')
  all() {
    return [];
  }

  @Post()
  create() {
    return {};
  }
}
