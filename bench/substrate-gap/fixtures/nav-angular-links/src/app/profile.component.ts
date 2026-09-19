import { Component } from '@angular/core';
import { Router } from '@angular/router';

@Component({ selector: 'app-profile', template: '<p>profile</p>' })
export class ProfileComponent {
  id = 'abc';
  constructor(private router: Router) {}
  get shareLink(): string {
    return `${window.location.origin}/connect?ref=${this.id}`;
  }
}
