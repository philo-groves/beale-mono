import { describe, expect, it } from 'vitest';
import { researchKitDefinition, researchKitResourceKey, selectedResearchKitCatalogAssets } from '../src/shared/researchKits';

const kit = researchKitDefinition('msrc');
const catalog = kit.resourceCatalog!;

describe('MSRC Windows resource catalog', () => {
  it('keeps Windows candidates unselected until the researcher records them', () => {
    expect(catalog.resources.filter((asset) => asset.attributes?.catalogGroup === 'Attack-scenario sandboxes').map((asset) => asset.value)).toEqual([
      'msedge.exe (renderer)',
      'MsMpEngCP.exe',
      'WinHTTP WPAD sandboxed process',
      'UtcDecoderHost.exe'
    ]);
    expect(selectedResearchKitCatalogAssets(catalog, [])).toEqual([]);
    expect(new Set(catalog.resources.map(researchKitResourceKey)).size).toBe(catalog.resources.length);
  });

  it('preserves selected checkout metadata and rejects invented catalog entries', () => {
    const terminal = catalog.resources.find((asset) => asset.value === 'https://github.com/microsoft/terminal')!;
    const key = researchKitResourceKey(terminal);
    const existing = {
      ...terminal,
      attributes: { source: 'msrc-windows', catalogNote: 'Old catalog copy', clonedDirectory: 'C:\\ExampleResearch\\terminal' }
    };
    expect(selectedResearchKitCatalogAssets(catalog, [existing], [key, key])).toEqual([
      expect.objectContaining({
        value: terminal.value,
        attributes: expect.objectContaining({
          researchKitId: 'msrc',
          catalogNote: terminal.attributes?.catalogNote,
          clonedDirectory: 'C:\\ExampleResearch\\terminal'
        })
      })
    ]);
    expect(() => selectedResearchKitCatalogAssets(catalog, [], ['unknown'])).toThrow('Unknown Research Kit resource');
  });
});
