import React, { lazy, Suspense, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { InitialAppShell } from './app/InitialAppShell';
import { devInstrumentation } from './devInstrumentation';
import './startup.css';

const App = lazy(() => import('./App').then((module) => ({ default: module.App })));

function RendererRoot(): React.JSX.Element {
  const [workbenchReady, setWorkbenchReady] = useState(false);
  useEffect(() => {
    const frame = window.requestAnimationFrame(() => setWorkbenchReady(true));
    return () => window.cancelAnimationFrame(frame);
  }, []);
  if (!workbenchReady) return <InitialAppShell />;
  return (
    <React.Profiler
      id="app"
      onRender={(_id, phase, actualDuration, baseDuration, startTime, commitTime) => {
        devInstrumentation.recordReactCommit('app', phase, actualDuration, baseDuration, startTime, commitTime);
      }}
    >
      <Suspense fallback={<InitialAppShell />}>
        <App />
      </Suspense>
    </React.Profiler>
  );
}

createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <RendererRoot />
  </React.StrictMode>
);
