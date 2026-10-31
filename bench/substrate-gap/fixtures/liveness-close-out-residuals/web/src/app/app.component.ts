import { Component } from '@angular/core';
import { AliasLoader } from './i18n/alias.loader';
import { AppErrorHandler } from './core/app.handler';

@Component({ selector: 'app-root', template: '<p></p>' })
export class AppComponent {
  constructor(private loader: AliasLoader, private errors: AppErrorHandler) {}
}
