import { expect, test } from 'vitest';
import { surfaceThreatDiagramConfig, type SurfaceDiagramTheme } from '../src/renderer/features/workspaces/surfaceThreatDiagramTheme';

function contrastRatio(first: string, second: string): number {
  const luminance = (hex: string): number => {
    const channels = [1, 3, 5].map((index) => Number.parseInt(hex.slice(index, index + 2), 16) / 255);
    return channels.reduce((sum, channel, index) => {
      const linear = channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
      return sum + linear * [0.2126, 0.7152, 0.0722][index]!;
    }, 0);
  };
  const values = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (values[0]! + 0.05) / (values[1]! + 0.05);
}

test('surface diagrams use a readable appearance-matched Mermaid theme', () => {
  const themes: SurfaceDiagramTheme[] = ['dark', 'light', 'cream', 'midnight'];
  const nodeColors = new Set<string>();
  for (const theme of themes) {
    const config = surfaceThreatDiagramConfig(theme);
    expect(config).toMatchObject({ securityLevel: 'strict', theme: 'base', look: 'neo',
      flowchart: { htmlLabels: false, curve: 'rounded' } });
    const colors = config.themeVariables as Record<string, string>;
    nodeColors.add(colors.mainBkg!);
    expect(colors.dropShadow).toBe('none');
    expect(contrastRatio(colors.nodeTextColor!, colors.mainBkg!)).toBeGreaterThan(7);
    expect(contrastRatio(colors.lineColor!, colors.clusterBkg!)).toBeGreaterThan(3);
  }
  expect(nodeColors.size).toBe(themes.length);
});
