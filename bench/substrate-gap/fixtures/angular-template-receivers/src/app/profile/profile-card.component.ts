import { Component, inject } from '@angular/core';
import { ThemeService } from '../core/theme.service';
import { AuthService } from '../core/auth.service';

@Component({
  selector: 'ks-profile-card',
  standalone: true,
  template: `
    <button type="button" (click)="theme.toggle()">Theme</button>
    <button type="button" (click)="auth.logout()">Sign out</button>
    <button type="button" (click)="auth.session.end()">End</button>
    @for (auth of sessions; track $index) {
      <button (click)="auth.stop()">Stop</button>
    }
  `,
})
export class ProfileCardComponent {
  readonly theme = inject(ThemeService);
  sessions: AuthService[] = [];
  constructor(public auth: AuthService) {}
}
