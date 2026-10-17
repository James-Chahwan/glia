import { Translation } from '@jsverse/transloco';

/** Imports the library, but is no TranslocoLoader: its getTranslation is only a name. */
export class FakeLoader {
  getTranslation(lang: string): Translation {
    return { lang } as unknown as Translation;
  }
}
