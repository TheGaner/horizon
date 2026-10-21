#!/usr/bin/env python3
"""Original Minecraft Java 26.3 codec multiplayer drop/inventory oracle.

Two original-codec clients join separate vanilla and Horizon dedicated servers
on the same vanilla world. Alice sends the exact ServerboundPlayerActionPacket
DROP_ITEM / DROP_ALL_ITEMS action sequences that the Minecraft client produces
for Q / held-Q / physical Ctrl+Q, plus ContainerClick THROW for inventory Q.
Bob observes the item entities. The input-to-action binding for left/right
Ctrl and toggle sprint is independently covered by a production Rust test;
a headless codec sender does not synthesize actual OS key events.
"""
import difflib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys

try:
    from . import lab, test_rust_parity as base, test_locator_bar_parity as loc
except ImportError:
    import lab
    import test_rust_parity as base
    import test_locator_bar_parity as loc

NAME = 'player-drop-parity'
SOURCE_PARTS = [
    Path(__file__).resolve(),
    lab.ROOT / 'mods/horizon/src/test/java/dev/horizonlab/parity/LocatorBarClient.java',
    lab.ROOT / 'tools/test_locator_bar_parity.py',
    lab.ROOT / 'tools/test_dedicated_server_parity.py',
    lab.ROOT / 'crates/horizon-client/src/game/lan_actions.rs',
    lab.ROOT / 'crates/horizon-client/src/game/commands/items.rs',
    lab.ROOT / 'crates/horizon-client/src/game/lan_crafting.rs',
    lab.ROOT / 'crates/horizon-client/src/lan/containers.rs',
    lab.ROOT / 'crates/horizon-client/src/lan/hub.rs',
]

STEP_SECONDS = 1.5
SCRIPT = [
    ('alice-joins', [], ['join Alice', 'cmd Alice gamemode survival']),
    ('bob-joins', [], ['join Bob', 'cmd Bob gamemode survival']),
    ('drop-start', [], [
        'state on', 'items on',
        'cmd Alice tp @s 0.5 -59 0.5',
        'cmd Bob tp @s 4.5 -59 0.5',
        'cmd Alice item replace entity @s hotbar.0 with minecraft:stick 8',
    ]),
    ('q-single', [], ['drop Alice single']),
    ('held-q-repeat-1', [], ['drop Alice single']),
    ('held-q-repeat-2', [], ['drop Alice single']),
    ('held-q-repeat-3', [], ['drop Alice single']),
    ('release-rearm', [], ['cmd Alice item replace entity @s hotbar.0 with minecraft:stick 8']),
    ('left-ctrl-q', [], ['drop Alice all']),
    ('right-ctrl-rearm', [], ['cmd Alice item replace entity @s hotbar.0 with minecraft:stick 8']),
    ('right-ctrl-q', [], ['drop Alice all']),
    ('latch-rearm', [], ['cmd Alice item replace entity @s hotbar.0 with minecraft:stick 8']),
    ('toggle-sprint-q', [], ['drop Alice single']),
    ('shift-q', [], ['drop Alice single']),
    ('empty-rearm', [], ['cmd Alice item replace entity @s hotbar.0 with minecraft:air']),
    ('empty-q', [], ['drop Alice single']),
    ('empty-ctrl-q', [], ['drop Alice all']),
    ('gui-rearm', [], ['cmd Alice item replace entity @s hotbar.0 with minecraft:stick 8']),
    ('gui-q', [], ['throw Alice single']),
    ('gui-ctrl-q', [], ['throw Alice all']),
    ('done', [], ['items off', 'state off']),
]


def result_lines(lines):
    """Compare observable player inventory and item spawn packets, not IDs/ticks."""
    keep = []
    for line in lines:
        if ' error ' in ' '+line+' ' or 'disconnected' in line:
            raise AssertionError('Vanilla-codec client reported error: ' + line)
        if any(word in line for word in (' slot ', ' inventory ', ' item-added', ' item-removed', ' sent ')):
            keep.append(line)
    return keep


def main():
    lock, jars, libraries = base.verify_artifacts()
    assert lock['minecraft']['version'] == '26.3'
    hashes = base.source_hashes(SOURCE_PARTS)
    build = lab.LOCAL / NAME
    build.mkdir(parents=True, exist_ok=True)
    report = build / 'report.json'
    report.unlink(missing_ok=True)
    classes = build / 'classes'
    classes.mkdir(exist_ok=True)
    oracle = lab.ROOT / 'mods/horizon/src/test/java/dev/horizonlab/parity/LocatorBarClient.java'
    dependencies = os.pathsep.join(map(str, [jars['server'], *libraries]))
    with (build/'compile-java.log').open('w') as log:
        subprocess.run(['javac','--release',str(lock['minecraft']['java_major']),'-proc:none',
                        '-nowarn','-sourcepath',str(lab.ROOT/'mods/horizon/src/test/java'),
                        '-cp',dependencies,'-d',str(classes),str(oracle)],
                       cwd=lab.ROOT, check=True, stdout=log, stderr=subprocess.STDOUT, timeout=180)
    with (build/'compile-rust.log').open('w') as log:
        subprocess.run(['cargo','build','--locked','--release','-p','horizon-client','--bin','horizon-client'],
                       cwd=lab.ROOT, check=True, stdout=log, stderr=subprocess.STDOUT, timeout=900)
    meta = json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1'], cwd=lab.ROOT, text=True))
    binary = Path(meta['target_directory'])/'release/horizon-client'
    vanilla_server = ['java','-Xmx2g','-Duser.timezone=UTC','-cp',dependencies,'net.minecraft.server.Main','nogui']
    horizon_server = [binary,'--resources',lab.LOCAL/'horizon-client/26.3','--dedicated','--nogui']
    client = ['java','-Xmx1g','-cp',str(classes)+os.pathsep+dependencies,'dev.horizonlab.parity.LocatorBarClient']
    # Reuse the checked-in original-codec two-player runner, isolated world and
    # op roster. No duplicate network protocol implementation.
    original_script, original_seconds = loc.SCRIPT, loc.STEP_SECONDS
    try:
        loc.SCRIPT, loc.STEP_SECONDS = SCRIPT, STEP_SECONDS
        world = loc.generate_world(vanilla_server, build)
        vanilla = loc.run_side('vanilla', vanilla_server, client, build, world)
        horizon = loc.run_side('horizon', horizon_server, client, build, world)
    finally:
        loc.SCRIPT, loc.STEP_SECONDS = original_script, original_seconds
    left, right = result_lines(vanilla), result_lines(horizon)
    # A menu THROW broadcasts an item spawn and an inventory slot update in
    # the same scripted stage. Their arrival order can differ by a tick;
    # compare semantic per-step packet multisets but retain the exact-order
    # difference in the report, not an unsupported full ordering claim.
    def same_step_events(lines):
        groups = {}
        for line in lines:
            who, label, rest = line.split(' ', 2)
            groups.setdefault((who, label), []).append(rest)
        return [(k, sorted(v)) for k, v in sorted(groups.items())]

    exact_order_equal = left == right
    if same_step_events(left) != same_step_events(right):
        diff = '\n'.join(difflib.unified_diff(left, right, 'vanilla', 'horizon', n=2, lineterm=''))
        raise AssertionError(f'{len(left)} vanilla vs {len(right)} Horizon player-event lines differ:\n{diff[:9000]}')
    for action in ('single', 'all', 'gui'):
        if not any(action in l for l in left):
            raise AssertionError(f'expected a decoded {action} action, got {len(left)} events')
    base.ensure_sources_unchanged(SOURCE_PARTS, hashes)
    lab.write_json(report, {
        'source_hashes':hashes,
        'schema':1,'minecraft':'26.3','java':subprocess.check_output(['java','--version'],text=True).splitlines()[0],
        'platform':platform.platform(),'events':len(left),'tests':len(SCRIPT),
        'transcript_sha256':lab.digest(build/'horizon.txt','sha256'),
        'packet_order_identical':exact_order_equal,
        'normalization':'within same scripted stage, item entity spawn and slot update may arrive in the opposite order',
        'scope':'Real multiplayer original Java-codec outbound drop action 5/4 and GUI THROW, inbound inventory and observed item-entity spawn packets compared on vanilla and Horizon servers. Physical OS left/right Ctrl, held repeat and toggle keymapping separately production-unit tested; sender drives their protocol-equivalent actions.',
    })
    print(f'PASS player-drop event parity: {len(left)} event lines, {len(SCRIPT)} scripted stages, two players; packet_order_identical={exact_order_equal}')


if __name__ == '__main__':
    try:
        main()
    except (AssertionError, subprocess.CalledProcessError) as e:
        sys.exit(f'{NAME}: {e}')
