import { Parent } from './base';

const svc = new Parent();

describe('parent', () => {
  it('counts', async () => {
    await svc.count();
  });
});
