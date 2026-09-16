import { init, LDClient } from 'launchdarkly-node-server-sdk';

const ldClient: LDClient = init('sdk-fixture-placeholder');

export async function checkout(userKey: string, cart: string[]) {
  const enabled = await ldClient.variation('new-checkout', { key: userKey }, false);
  return enabled ? { flow: 'new', cart } : { flow: 'legacy', cart };
}
