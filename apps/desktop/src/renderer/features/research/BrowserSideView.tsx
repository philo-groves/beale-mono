import { useEffect, useRef, useState } from 'react';
import type { FormEvent, JSX } from 'react';
import type { WebviewTag } from 'electron';

export function browserNavigationUrl(input: string): string | null {
  const value = input.trim();
  if (!value) return null;
  try {
    const url = new URL(value.includes('://') ? value : `https://${value}`);
    return url.protocol === 'https:' || url.protocol === 'http:' ? url.href : null;
  } catch {
    return null;
  }
}

export function BrowserSideView({ visible, partition, label }: { visible: boolean; partition: string; label: string }): JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const guestRef = useRef<WebviewTag | null>(null);
  const [address, setAddress] = useState('');
  const [error, setError] = useState('');

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return undefined;
    const guest = document.createElement('webview') as WebviewTag;
    guest.setAttribute('src', 'about:blank');
    guest.setAttribute('partition', partition);
    guest.setAttribute('class', 'research-browser-webview');
    guest.setAttribute('aria-label', `${label} browser page`);
    const onNavigate = (event: Event): void => {
      const url = (event as Event & { url?: string }).url;
      if (url && url !== 'about:blank') {
        setAddress(url);
        setError('');
      }
    };
    const onFail = (event: Event): void => {
      const detail = event as Event & { errorCode?: number; errorDescription?: string };
      if (detail.errorCode !== -3) setError(detail.errorDescription ?? 'Page failed to load.');
    };
    guest.addEventListener('did-navigate', onNavigate);
    guest.addEventListener('did-navigate-in-page', onNavigate);
    guest.addEventListener('did-fail-load', onFail);
    container.appendChild(guest);
    guestRef.current = guest;
    return () => {
      guestRef.current = null;
      guest.removeEventListener('did-navigate', onNavigate);
      guest.removeEventListener('did-navigate-in-page', onNavigate);
      guest.removeEventListener('did-fail-load', onFail);
      guest.remove();
    };
  }, [partition]);

  useEffect(() => {
    guestRef.current?.setAttribute('aria-label', `${label} browser page`);
  }, [label]);

  const navigate = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    const url = browserNavigationUrl(address);
    if (!url) {
      setError('Enter an HTTP or HTTPS URL.');
      return;
    }
    setError('');
    void guestRef.current?.loadURL(url).catch((cause: unknown) => {
      setError(cause instanceof Error ? cause.message : 'Page failed to load.');
    });
  };

  return (
    <section className="research-browser" hidden={!visible} aria-label={`${label} browser`}>
      <form className="research-browser-address" onSubmit={navigate}>
        <input
          type="text"
          inputMode="url"
          aria-label="Browser URL"
          placeholder="Enter a URL"
          value={address}
          onChange={(event) => setAddress(event.target.value)}
        />
      </form>
      {error ? <p className="research-browser-error" role="alert">{error}</p> : null}
      <div className="research-browser-page" ref={containerRef} />
    </section>
  );
}
