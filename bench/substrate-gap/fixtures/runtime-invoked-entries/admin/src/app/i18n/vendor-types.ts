/** An in-repo interface that happens to share Transloco's name. */
export interface TranslocoLoader {
  getTranslation(lang: string): unknown;
}
