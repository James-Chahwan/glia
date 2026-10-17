import { AuthService } from '../core/auth.service';

/** A project-local `inject`: not Angular's, so no DI. */
function inject<T>(token: T): T {
  return token;
}

export function probe(): boolean {
  return inject(AuthService) !== null;
}
