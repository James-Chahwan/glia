import { Controller, Get, Put } from '@nestjs/common';
import { ApiPrefix, RouteKey } from './route-keys';

@Controller(RouteKey.User)
export class UserController {
  @Put('me')
  updateMe() {}

  @Get(`${RouteKey.Album}/shared`)
  sharedAlbums() {}
}

@Controller(ApiPrefix.Admin)
export class AdminController {
  @Get('stats')
  stats() {}
}
