import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';

// Spec, Visuals: "the existing tokens and controls.css; the hard-coded hex
// colours in AssetDetail and the sync plan view move to tokens". Held for every
// component of the Assets workspace, so a new one cannot bring one back.
// Each task that adds an Assets component adds its file here.
const GUARDED = [
  'AssetsPanel.svelte',
  'AssetList.svelte',
  'AssetDetail.svelte',
  'AssetEditor.svelte',
  'SyncPlanView.svelte',
  'HostStrip.svelte',
  'Badge.svelte',
  'IdentityRow.svelte',
  'RowName.svelte',
  'AssetsInbox.svelte',
  'QueryInput.svelte',
  'Inspector.svelte',
  'AssetInspector.svelte',
  'JobChip.svelte',
  'CatalogChip.svelte',
  'AssetsFooter.svelte',
  'AssetsRail.svelte',
  'AssetsWorkspace.svelte',
  'ChangesetCard.svelte',
  'ChangesetDetail.svelte',
  'DiffView.svelte',
  'DriftPanel.svelte',
  'AssetsLayers.svelte',
  'LayerInspector.svelte',
  'LayerChangeForm.svelte',
  'AssetsHosts.svelte',
  'HostInspector.svelte',
  // The dialogs the workspace opens (Assets M6, Task 15).
  'ImportDialog.svelte',
  'NewAssetDialog.svelte',
  'SecretsPanel.svelte',
  'AuthorSessionDialog.svelte',
  'LintAllDialog.svelte',
];

const COLOR_LITERAL = /#[0-9a-fA-F]{3,8}\b|\b(?:rgb|rgba|hsl|hsla)\(/g;

const styleOf = (src: string) => src.match(/<style[^>]*>([\s\S]*?)<\/style>/)?.[1] ?? '';

describe('Assets components colour only through tokens', () => {
  for (const file of GUARDED) {
    it(file, () => {
      const css = styleOf(readFileSync(`src/lib/${file}`, 'utf8'));
      expect(css.match(COLOR_LITERAL) ?? []).toEqual([]);
      expect(css.match(/:\s*(white|black)\b/g) ?? []).toEqual([]);
    });
  }
});
