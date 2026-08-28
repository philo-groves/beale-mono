import { consumeAppServerLaunchEnvironment } from './launchEnvironment.js';

consumeAppServerLaunchEnvironment();
await import('./trayMain.js');
