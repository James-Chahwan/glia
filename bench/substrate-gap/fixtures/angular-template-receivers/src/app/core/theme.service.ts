import { Injectable, signal } from '@angular/core';

@Injectable({ providedIn: 'root' })
export class ThemeService {
  readonly dark = signal(false);
  get isDark(): boolean { return this.dark(); }
  get isLight(): boolean { return !this.dark(); }
  toggle(): void { this.dark.update((d) => !d); }
  set(dark: boolean): void { this.dark.set(dark); }
}
