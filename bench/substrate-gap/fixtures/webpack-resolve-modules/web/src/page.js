import { getUser } from 'common/api';

export function show() {
  return getUser(1);
}
