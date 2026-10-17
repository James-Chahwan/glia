import { Component } from '@angular/core';
import { LocalLoader } from './i18n/local.loader';

@Component({ selector: 'admin-root', template: '<p></p>' })
export class AdminComponent {
  constructor(private local: LocalLoader) {}
}
