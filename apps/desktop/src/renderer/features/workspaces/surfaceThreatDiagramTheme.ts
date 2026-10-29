import type { MermaidConfig } from 'mermaid';

export type SurfaceDiagramTheme = 'dark' | 'light' | 'cream' | 'midnight';

interface DiagramPalette {
  background: string;
  node: string;
  nodeBorder: string;
  group: string;
  groupBorder: string;
  label: string;
  text: string;
  edge: string;
}

// Rendered SVG images do not inherit the app's CSS variables.
const PALETTES: Record<SurfaceDiagramTheme, DiagramPalette> = {
  dark: {
    background: '#202020', node: '#292e31', nodeBorder: '#71858d', group: '#252a2d',
    groupBorder: '#52636a', label: '#30383b', text: '#f0f4f5', edge: '#a8b9bf'
  },
  light: {
    background: '#f1f3f5', node: '#ffffff', nodeBorder: '#8da4af', group: '#e8eef1',
    groupBorder: '#b9cbd2', label: '#f7fafb', text: '#1e2b32', edge: '#5f7783'
  },
  cream: {
    background: '#f1e7d8', node: '#fffaf1', nodeBorder: '#9c8b73', group: '#eee2d0',
    groupBorder: '#c3ad90', label: '#faf2e7', text: '#30271e', edge: '#786b58'
  },
  midnight: {
    background: '#182a46', node: '#1b3047', nodeBorder: '#7799b1', group: '#14253a',
    groupBorder: '#4a6882', label: '#22384e', text: '#eef5fb', edge: '#a5c0d4'
  }
};

export function surfaceThreatDiagramConfig(theme: SurfaceDiagramTheme): MermaidConfig {
  const palette = PALETTES[theme];
  return {
    startOnLoad: false,
    securityLevel: 'strict',
    suppressErrorRendering: true,
    theme: 'base',
    look: 'neo',
    fontFamily: 'Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
    maxTextSize: 50_000,
    maxEdges: 200,
    themeVariables: {
      background: palette.background,
      primaryColor: palette.node,
      primaryBorderColor: palette.nodeBorder,
      primaryTextColor: palette.text,
      secondaryColor: palette.node,
      secondaryBorderColor: palette.nodeBorder,
      secondaryTextColor: palette.text,
      tertiaryColor: palette.group,
      tertiaryBorderColor: palette.groupBorder,
      tertiaryTextColor: palette.text,
      mainBkg: palette.node,
      nodeBkg: palette.node,
      nodeBorder: palette.nodeBorder,
      nodeTextColor: palette.text,
      clusterBkg: palette.group,
      clusterBorder: palette.groupBorder,
      edgeLabelBackground: palette.label,
      lineColor: palette.edge,
      arrowheadColor: palette.edge,
      textColor: palette.text,
      titleColor: palette.text,
      fontFamily: 'Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
      fontSize: '15px',
      strokeWidth: 1.5,
      useGradient: false,
      dropShadow: 'none'
    },
    themeCSS: `
      .node rect { rx: 10px; ry: 10px; }
      .cluster rect { rx: 16px; ry: 16px; }
      .edgeLabel rect { rx: 5px; ry: 5px; opacity: 1 !important; }
      .node .label text { font-weight: 500; }
      .cluster-label text { font-weight: 600; }
    `,
    flowchart: {
      htmlLabels: false,
      curve: 'rounded',
      nodeSpacing: 48,
      rankSpacing: 62,
      padding: 18
    }
  };
}
