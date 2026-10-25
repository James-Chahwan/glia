import { Controller, Put } from '@nestjs/common';
import { ApiOperation } from '@nestjs/swagger';
import { RouteKey } from './route-keys';

@Controller(RouteKey.User)
export class UserController {
  @ApiOperation({ operationId: 'updateMe' })
  @Put('me')
  update() {}
}
