import { environment } from '../environments/environment';

export const ADMIN_BASE = environment.apiUrl.replace('/api', '') + '/admin';
