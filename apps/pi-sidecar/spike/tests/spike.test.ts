import { describe, it, expect } from 'vitest';
import { runSpike } from '../src/spike.js';

describe('Pi SDK bundle spike', () => {
  // This test PROVES the SDK bundles and createAgentSession is callable.
  // If the SDK's API changes or it can't be imported in a Node ESM context,
  // this test fails — which is the signal to update the spike + Plan 4.
  it('createAgentSession is callable and returns a session object', async () => {
    const result = await runSpike();
    expect(result.ok).toBe(true);
    expect(result.session).toBeDefined();
  }, 30000); // 30s timeout — SDK init may be slow
});
