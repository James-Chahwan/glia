import { Translation, TranslocoLoader as TL } from '@jsverse/transloco';

/** Implements Transloco's loader under an import alias. */
export class AliasLoader implements TL {
  getTranslation(lang: string): Promise<Translation> {
    return Promise.resolve({ lang } as unknown as Translation);
  }
}
