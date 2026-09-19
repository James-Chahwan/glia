import { Component } from '@angular/core';
import { Router } from '@angular/router';

@Component({ selector: 'app-login', template: '<p>login</p>' })
export class LoginComponent {
  constructor(private router: Router) {}
  ok() {
    this.router.navigate(['/home']);
  }
  verify() {
    this.router.navigateByUrl('/verify-email');
  }
  back(p: string) {
    this.router.navigate([p]);
  }
}
