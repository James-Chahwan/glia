import { ApplicationConfig } from '@angular/core';
import { HTTP_INTERCEPTORS, provideHttpClient, withInterceptors } from '@angular/common/http';
import { provideRouter } from '@angular/router';
import { routes } from './app.routes';
import { jwtInterceptor } from './core/interceptors/jwt.interceptor';
import { AuthInterceptor } from './core/interceptors/auth.interceptor';
import { provideI18n } from './core/i18n';

export const appConfig: ApplicationConfig = {
  providers: [
    provideRouter(routes),
    provideHttpClient(withInterceptors([jwtInterceptor])),
    { provide: HTTP_INTERCEPTORS, useClass: AuthInterceptor, multi: true },
    provideI18n(),
  ],
};
