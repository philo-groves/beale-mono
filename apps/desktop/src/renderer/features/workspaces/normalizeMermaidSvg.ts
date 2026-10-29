/** Mermaid may emit HTML void tags inside SVG foreignObject labels. Serialize through
 * an inert HTML template so SVG image decoders receive well-formed XML. */
export function normalizeMermaidSvg(svg: string): string {
  const parser = new DOMParser();
  const validSvg = (candidate: string): boolean => {
    const parsed = parser.parseFromString(candidate, 'image/svg+xml');
    return parsed.documentElement.localName === 'svg' && !parsed.querySelector('parsererror');
  };
  if (validSvg(svg)) return svg;
  const template = document.createElement('template');
  template.innerHTML = svg;
  const element = template.content.querySelector('svg');
  if (!element || element.namespaceURI !== 'http://www.w3.org/2000/svg') {
    throw new Error('Mermaid did not produce an SVG diagram.');
  }
  const normalized = new XMLSerializer().serializeToString(element);
  if (!validSvg(normalized)) {
    throw new Error('Mermaid produced invalid SVG.');
  }
  return normalized;
}
