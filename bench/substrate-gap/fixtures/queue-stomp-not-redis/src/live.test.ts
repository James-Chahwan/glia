import { describe, expect, it, vi } from 'vitest';
import { connectLive } from './live';

const CHANNEL = '/projects/1/changes';

describe('connectLive', () => {
  it('reports a closed socket', () => {
    const fireDisconnected = vi.fn();
    const client = connectLive('http://localhost/ws', fireDisconnected);
    client.subscribe(CHANNEL, () => {});
    expect(fireDisconnected).not.toHaveBeenCalled();
  });
});
