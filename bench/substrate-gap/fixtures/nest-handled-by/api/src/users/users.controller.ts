import { Body, Controller, Get, Param, Put, UseGuards } from '@nestjs/common';

class AuthGuard {}

@Controller('users')
export class UsersController {
  @Get(':id')
  getUser(@Param('id') id: string) {
    return { id };
  }

  @Put('me')
  @UseGuards(AuthGuard)
  async updateMe(
    @Body() body: { name: string },
  ): Promise<{ name: string }> {
    return body;
  }
}
