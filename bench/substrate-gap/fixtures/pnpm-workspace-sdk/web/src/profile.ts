import { updateMyUser } from '@acme/sdk';
import { getServerVersion } from '@acme/sdk/fetch-client';
import { formatName } from '@acme/ui';
import { generate } from '@acme/gen';
import { debounce } from 'lodash';

export async function saveProfile(name: string): Promise<Response> {
  return updateMyUser({ name: formatName(name) });
}

export async function showVersion(): Promise<Response> {
  return getServerVersion();
}

export const later = debounce(() => generate(), 10);
