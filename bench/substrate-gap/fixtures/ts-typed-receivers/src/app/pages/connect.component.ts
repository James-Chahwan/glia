import { Component, OnInit } from '@angular/core';
import { AuthService } from '../core/auth.service';

@Component({ selector: 'app-connect', standalone: true, template: '<p></p>' })
export class ConnectComponent implements OnInit {
  constructor(private readonly auth: AuthService) {}

  ngOnInit(): void {
    if (!this.auth.isAuthenticated) {
      this.auth.session = 'guest';
    }
  }
}
