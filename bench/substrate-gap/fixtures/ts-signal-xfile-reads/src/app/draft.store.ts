import { Injectable, signal, computed } from '@angular/core';

@Injectable({ providedIn: 'root' })
export class DraftStore {
  draft = signal<string>('');
  length = computed(() => this.draft().length);
  save(): void {}
}
