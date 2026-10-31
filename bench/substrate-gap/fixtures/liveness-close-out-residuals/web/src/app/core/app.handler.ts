import { BaseHandler } from './base.handler';

/** Angular calls handleError through the in-repo base. */
export class AppErrorHandler extends BaseHandler {
  override handleError(error: unknown): void {
    console.error(this.tag, error);
  }
}
