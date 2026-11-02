import { Component, inject } from '@angular/core';
import { Router } from '@angular/router';

@Component({ selector: 'app-items-page', template: '<ul></ul>' })
export class ItemsPageComponent {
  private readonly router = inject(Router);

  open(id: string): void {
    this.router.navigate(['/app/items', id]);
  }
}
