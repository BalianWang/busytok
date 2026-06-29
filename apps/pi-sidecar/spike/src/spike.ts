// Minimal spike: import the SDK's createAgentSession and call it.
// If this file compiles, bundles, and runs without error, the SDK is
// usable in a Node stdio context. Real turn_auto wiring is Plan 4.

import { createAgentSession } from '@earendil-works/pi-coding-agent';

export async function runSpike(): Promise<{ ok: true; session: unknown }> {
  // createAgentSession signature varies by SDK version; the spike just
  // needs to prove the function is callable and returns a session object.
  // We pass a minimal config; real config wiring is Plan 4.
  const session = await createAgentSession({
    model: 'deepseek-chat',
    workingDir: process.cwd(),
  });
  return { ok: true, session };
}

// When run directly (node dist/spike.bundle.js), execute the spike.
if (import.meta.url === `file://${process.argv[1]}`) {
  runSpike()
    .then((r) => {
      console.log('SPIKE PASS', r.ok);
      process.exit(0);
    })
    .catch((e) => {
      console.error('SPIKE FAIL', e);
      process.exit(1);
    });
}
