import { Injectable } from '@angular/core';

export class Session { end(): void {} }

@Injectable({ providedIn: 'root' })
export class AuthService {
  readonly session = new Session();
  logout(): void {}
  stop(): void {}
}
