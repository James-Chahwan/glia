import { hello } from '@acme/kit';
import { slugify } from '@acme/kit/utils/slug';

export function label() {
  return slugify(hello());
}
