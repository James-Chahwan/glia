import { ErrorHandler } from '@angular/core';

/** An in-repo intermediate base over Angular's ErrorHandler. */
export class BaseHandler extends ErrorHandler {
  protected readonly tag = 'app';
}
