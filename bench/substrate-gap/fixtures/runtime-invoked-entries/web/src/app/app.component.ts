import { Component } from '@angular/core';
import { AppErrorHandler } from './core/error.handler';
import { FakeLoader } from './i18n/fake.loader';
import { HttpLoader } from './i18n/http.loader';

@Component({ selector: 'app-root', template: '<p></p>' })
export class AppComponent {
  constructor(
    private loader: HttpLoader,
    private fake: FakeLoader,
    private errors: AppErrorHandler,
  ) {}
}
