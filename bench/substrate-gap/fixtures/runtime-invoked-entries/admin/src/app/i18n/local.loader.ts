import { Translation } from '@jsverse/transloco';
import { TranslocoLoader } from './vendor-types';

/** Imports the library, but implements the in-repo look-alike interface. */
export class LocalLoader implements TranslocoLoader {
  getTranslation(lang: string): Translation {
    return { lang } as unknown as Translation;
  }
}
