import { ErrorHandler } from '@angular/core';

/** Angular calls handleError on the app's ErrorHandler. */
export class AppErrorHandler implements ErrorHandler {
  handleError(error: unknown): void {
    console.error(error);
  }
}
