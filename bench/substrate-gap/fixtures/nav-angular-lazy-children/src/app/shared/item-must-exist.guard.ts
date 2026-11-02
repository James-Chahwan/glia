import { Injectable } from '@angular/core';
import { CanActivate, Router } from '@angular/router';

@Injectable({ providedIn: 'root' })
export class ItemMustExistGuard implements CanActivate {
  constructor(private readonly router: Router) {}

  canActivate(): boolean {
    const found = false;
    if (!found) {
      this.router.navigate(['/404']);
    }
    return found;
  }
}
