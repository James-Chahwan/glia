export class UrlBuilder {
  buildWsUrl(path: string): string {
    return new URL(path, 'ws://localhost:8080/').toString();
  }
}
