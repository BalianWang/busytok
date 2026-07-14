import { JsonRpcServer } from './rpc.js';
import { SessionPool } from './session_pool.js';
import { initializeHandler } from './handlers/initialize.js';
import { healthHandlerWithPool } from './handlers/health.js';
import { shutdownHandler } from './handlers/shutdown.js';
import { turnAutoHandlerWithPool } from './handlers/turn_auto.js';
import { prepareHibernateHandlerWithPool } from './handlers/prepare_hibernate.js';
import { closeHandlerWithPool } from './handlers/close.js';
import { cancelHandlerWithPool } from './handlers/cancel.js';
import { activateHandlerWithPool } from './handlers/activate.js';

const maxHot = parseInt(process.env.BUSYTOK_SIDECAR_MAX_HOT_SESSIONS ?? '3', 10);
const pool = new SessionPool(maxHot);
const server = new JsonRpcServer();

server.registerHandler('adapter.initialize', initializeHandler);
server.registerHandler('adapter.health', healthHandlerWithPool(pool));
server.registerHandler('adapter.shutdown', shutdownHandler);
server.registerHandler('session.turn_auto', turnAutoHandlerWithPool(pool));
server.registerHandler('session.prepare_hibernate', prepareHibernateHandlerWithPool(pool));
server.registerHandler('session.close', closeHandlerWithPool(pool));
server.registerHandler('session.cancel', cancelHandlerWithPool(pool));
server.registerHandler('session.activate', activateHandlerWithPool(pool));

server.onStop(() => process.exit(0));
server.start();
