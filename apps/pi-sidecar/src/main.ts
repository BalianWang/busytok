import { JsonRpcServer } from './rpc.js';
import { initializeHandler } from './handlers/initialize.js';
import { healthHandler } from './handlers/health.js';
import { shutdownHandler } from './handlers/shutdown.js';
import { turnAutoHandler } from './handlers/turn_auto.js';
import { prepareHibernateHandler } from './handlers/prepare_hibernate.js';
import { closeHandler } from './handlers/close.js';

const server = new JsonRpcServer();

server.registerHandler('adapter.initialize', initializeHandler);
server.registerHandler('adapter.health', healthHandler);
server.registerHandler('adapter.shutdown', shutdownHandler);
server.registerHandler('session.turn_auto', turnAutoHandler);
server.registerHandler('session.prepare_hibernate', prepareHibernateHandler);
server.registerHandler('session.close', closeHandler);

// Exit the process when the server stops (stdin closed or adapter.shutdown).
server.onStop(() => process.exit(0));

server.start();
