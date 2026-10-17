import { Injectable } from '@angular/core';
import { Translation, TranslocoLoader } from '@jsverse/transloco';

/** Transloco calls getTranslation on the registered loader. */
@Injectable({ providedIn: 'root' })
export class HttpLoader implements TranslocoLoader {
  getTranslation(lang: string): Promise<Translation> {
    return Promise.resolve({ lang } as unknown as Translation);
  }

  getTranslationLater(lang: string): string {
    return lang;
  }
}
