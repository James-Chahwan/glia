import { Module } from '@nestjs/common';
import { RouterModule } from '@nestjs/core';
import { AdminModule } from './admin/admin.module';
import { HealthModule } from './health/health.module';

@Module({
  imports: [
    AdminModule,
    HealthModule,
    RouterModule.register([{ path: 'backoffice', module: AdminModule }]),
  ],
})
export class AppModule {}
