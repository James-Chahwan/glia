import { Component } from '@angular/core';

@Component({ selector: 'app-chip', standalone: true, template: '<span></span>' })
export class Chip {
  active = false;

  filter(query: string): boolean {
    return query.length > 0;
  }
}
