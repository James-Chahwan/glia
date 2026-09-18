import { Body, Controller, Get, Param, Post } from '@nestjs/common';
import { ApiOperation, ApiResponse, ApiTags } from '@nestjs/swagger';

@ApiTags('users')
@Controller('users')
export class UsersController {
  @Get(':id')
  @ApiOperation({ summary: 'Get a user', operationId: 'getUser' })
  @ApiResponse({ status: 200, description: 'The user' })
  findOne(@Param('id') id: string) {
    return { id };
  }

  @Post()
  create(@Body() body: object) {
    return body;
  }
}
