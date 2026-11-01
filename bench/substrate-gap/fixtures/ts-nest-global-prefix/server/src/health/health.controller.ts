import { Controller, Get } from '@nestjs/common';

@Controller()
export class HealthController {
  @Get('health')
  health() {
    return { ok: true };
  }

  @Get('version')
  version() {
    return '1';
  }
}
