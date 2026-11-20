"""Render synthetic Beale views in all appearance themes without contacting a host.

Run from beale-v2 after building with --target-dir target/ui-preview.
Pillow is optional; install it only if contact sheets are wanted.
"""
import argparse
import os
from pathlib import Path
import subprocess

VIEWS = [
    'home', 'empty', 'research', 'session', 'canvas', 'chat',
    'workspace', 'rules', 'surfaces', 'surface-detail', 'surface-manage', 'findings',
    'import', 'create-general', 'create-surfaces', 'create-rules',
    'features', 'plugins', 'kits', 'fleet', 'vms', 'research-tools',
    'automations', 'tracking', 'providers', 'custom', 'safety', 'guardian',
    'appearance', 'typography', 'archive', 'archive-chats', 'dialog', 'menu', 'picker',
    'anthropic', 'openrouter', 'xai', 'zai', 'api-key', 'custom-editor',
    'graph-editor', 'model-menu', 'goal-menu', 'surface-add', 'surface-delete',
    'relationship', 'workspace-name', 'workspace-rule', 'feature-detail',
    'plugin-add', 'plugin-detail', 'fleet-connect', 'fleet-import', 'fleet-detail',
    'safety-rule', 'finding-detail', 'campaign',
]

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', default='target/ui-preview/debug/beale-desktop-app.exe' if os.name == 'nt' else 'target/ui-preview/debug/beale-desktop-app')
    parser.add_argument('--out', default='target/ui-audit')
    parser.add_argument('--themes', nargs='+', default=['light', 'dark', 'cream', 'midnight'])
    parser.add_argument('--views', nargs='+', default=VIEWS)
    parser.add_argument('--size', default='1440x960')
    parser.add_argument('--at', default='1',
                        help='virtual seconds before the snapshot, so width-driven layout settles')
    args = parser.parse_args()
    output = Path(args.out)
    output.mkdir(parents=True, exist_ok=True)
    for theme in args.themes:
        for view in args.views:
            env = dict(os.environ, BEALE_UI_PREVIEW=view, BEALE_UI_THEME=theme,
                       BEALE_V2_HOME=str(output.resolve() / 'isolated-home'),
                       BEALE_V2_APP_SERVER_ADDR='127.0.0.1:9')
            result = subprocess.run([args.binary, '--snap', str(output / f'{view}-{theme}.png'),
                                     '--scale', '1', '--size', args.size,
                                     '--at', args.at], env=env,
                                    capture_output=True, text=True, timeout=30)
            if result.returncode:
                raise SystemExit(result.stderr)
            print(f'{theme}: {view}', flush=True)
        try:
            from PIL import Image, ImageDraw
        except ImportError:
            continue
        for offset in range(0, len(args.views), 8):
            views = args.views[offset:offset + 8]
            sheet = Image.new('RGB', (1440, ((len(views) + 1) // 2) * 504), '#d7dce0')
            draw = ImageDraw.Draw(sheet)
            for index, view in enumerate(views):
                x, y = index % 2 * 720, index // 2 * 504
                draw.text((x + 12, y + 6), f'{view} / {theme}', fill='#17231f')
                with Image.open(output / f'{view}-{theme}.png') as shot:
                    shot.thumbnail((720, 480))
                    sheet.paste(shot, (x, y + 24))
            sheet.save(output / f'sheet-{theme}-{offset // 8 + 1}.png')

if __name__ == '__main__':
    main()
