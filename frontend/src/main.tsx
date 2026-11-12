/** === PREACT ENTRYPOINT ===
 *
 * PURPOSE:
 * Mounts the embedded `imgforge` Web Canvas SPA into `#app`.
 */

import { render } from 'preact';
import { App } from './App';
import './styles/app.css';

const root = document.getElementById('app');
if (root === null) {
  throw new Error('Root mount element #app not found in index.html');
}

render(<App />, root);
