import { inject } from '@angular/core';
import { CanActivateFn, Router } from '@angular/router';
import { AuthService } from './auth.service';

/** Injection options: a value passed to inject, not a token. */
export const OPTIONAL = { optional: true } as const;

export const authGuard: CanActivateFn = () => {
  const auth = inject(AuthService, OPTIONAL);
  const router = inject(Router);
  if (auth.isLoggedOut) {
    return router.url !== '/login';
  }
  return auth.isAuthenticated || auth.ready;
};

export function describeAuth(auth: AuthService): string {
  return typeof auth.check;
}
