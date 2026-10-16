import { environment } from '../../environments/environment';

export function adminBase(): string {
  return environment.apiUrl.replace(/\/api$/, '') + '/admin';
}
