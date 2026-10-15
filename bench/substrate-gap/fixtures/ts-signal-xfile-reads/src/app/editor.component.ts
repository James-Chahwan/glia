import { Component, inject } from '@angular/core';
import { DraftStore } from './draft.store';

@Component({ selector: 'app-editor', template: '' })
export class EditorComponent {
  private store = inject(DraftStore);
  constructor(private other: DraftStore) {}

  show(): string {
    this.store.save();
    const n = this.store.length();
    return this.other.draft();
  }
}
