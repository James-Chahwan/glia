import { Injectable } from '@angular/core';

@Injectable({ providedIn: 'root' })
export class AuthService {
  private token: string | null = null;
  ready = false;

  get isAuthenticated(): boolean {
    return this.token !== null;
  }

  get isLoggedOut(): boolean {
    return this.token === null;
  }

  set session(t: string) {
    this.token = t;
  }

  get unusedFlag(): boolean {
    return false;
  }

  check(): boolean {
    return this.ready;
  }
}
