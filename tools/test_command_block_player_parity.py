#!/usr/bin/env python3
"""Compare 26.3 command-block chain/selector/repeating effects as *players* see them.

Unlike the command block NBT/server-only chain oracle, this runs two original
Minecraft protocol-codec players against each real vanilla and Horizon
dedicated server and compares score updates and chats delivered on the wire.
The narrow scripted observer scope is not comprehensive Brigadier coverage.
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

NAME = 'command-block-player-parity'
SOURCE_PARTS = [
    Path(__file__).resolve(),
    lab.ROOT / 'mods/horizon/src/test/java/dev/horizonlab/parity/LocatorBarClient.java',
    lab.ROOT / 'tools/test_locator_bar_parity.py',
    lab.ROOT / 'tools/test_dedicated_server_parity.py',
    lab.ROOT / 'crates/horizon-client/src/game/command_blocks.rs',
    lab.ROOT / 'crates/horizon-client/src/game/commands.rs',
    lab.ROOT / 'crates/horizon-client/src/game/lan_actions.rs',
    lab.ROOT / 'crates/horizon-client/src/game/lan.rs',
    lab.ROOT / 'crates/horizon-client/src/game/scoreboard.rs',
    lab.ROOT / 'crates/horizon-client/src/game/commands/data.rs',
    lab.ROOT / 'crates/horizon-client/src/scoreboard.rs',
]

def configure_chain():
    commands = [
        ('setblock 6 -59 0 minecraft:chain_command_block[facing=east,conditional=true]',
         'data merge block 6 -59 0 {Command:"execute as @a[tag=!cbobserver,distance=..32] run scoreboard players add @s cbview 1",auto:1b}'),
        ('setblock 7 -59 0 minecraft:chain_command_block[facing=east,conditional=true]',
         'data merge block 7 -59 0 {Command:"execute if score Alice cbview matches 999 run scoreboard players add Alice cbview 10",auto:1b}'),
        ('setblock 8 -59 0 minecraft:chain_command_block[facing=east,conditional=true]',
         'data merge block 8 -59 0 {Command:"scoreboard players add Alice cbview 100",auto:1b}'),
        ('setblock 9 -59 0 minecraft:chain_command_block[facing=east,conditional=false]',
         'data merge block 9 -59 0 {Command:"execute as @a[tag=cbobserver,distance=..32] run scoreboard players add @s cbview 2",auto:1b}'),
        ('setblock 10 -59 0 minecraft:chain_command_block[facing=east,conditional=true]',
         'data merge block 10 -59 0 {Command:"execute as @a[distance=..32] run scoreboard players add @s cbview 3",auto:1b}'),
    ]
    return [part for pair in commands for part in pair]

SCRIPT = [
    ('alice-joins', [], ['join Alice', 'cmd Alice gamemode survival']),
    ('watch', [], ['state on', 'chat on']),
    # The objective exists and holds both score entries *before* Bob arrives.
    # Original Minecraft sends the full tracked objective snapshot to a
    # late-joining player, not only future score mutations.
    ('objective', ['scoreboard objectives add cbview dummy',
                   'scoreboard objectives setdisplay sidebar cbview',
                   'scoreboard players set Alice cbview 0',
                   'scoreboard players set Bob cbview 0'], []),
    ('bob-joins-late', [], ['join Bob', 'cmd Bob gamemode survival']),
    ('position', ['tag Bob add cbobserver'],
     ['cmd Alice tp @s 0.5 -59 0.5', 'cmd Bob tp @s 3.5 -59 0.5']),
    ('chain-configure', configure_chain(), []),
    ('chain-trigger', [
        'setblock 5 -59 0 minecraft:command_block[facing=east]',
        'data merge block 5 -59 0 {Command:"scoreboard players add Alice cbview 1",auto:1b}',
    ], []),
    ('chain-settle', [], []),
    ('chain-observe', [], ['cmd Alice scoreboard players get Alice cbview',
                           'cmd Bob scoreboard players get Bob cbview']),
    ('repeat-freeze', ['tick freeze'], []),
    ('repeat-arm', [
        'setblock 11 -59 0 minecraft:repeating_command_block[facing=east]',
        'data merge block 11 -59 0 {Command:"scoreboard players add Bob cbview 1",auto:0b}',
        'data merge block 11 -59 0 {auto:1b}',
    ], []),
    ('repeat-step', ['tick step 3'], []),
    ('repeat-settle', [], []),
    ('repeat-observe', [], ['cmd Alice scoreboard players get Bob cbview']),
    ('repeat-stop', ['data merge block 11 -59 0 {auto:0b}', 'tick unfreeze'], []),
    ('selector-rearm', ['data merge block 5 -59 0 {auto:0b}',
                        'data merge block 5 -59 0 {Command:"execute as @a[tag=!cbobserver,distance=..32] run scoreboard players add @s cbview 4",auto:1b}'], []),
    ('selector-settle', [], []),
    ('selector-observe', [], ['cmd Alice scoreboard players get Alice cbview',
                              'cmd Bob scoreboard players get Bob cbview']),
    ('watch-off', [], ['chat off', 'state off']),
]

def filtered(lines):
    """Wire-observed scoreboard packets and numeric command replies.

    Admin feedback's rich translated text is a separate, wider packet gate:
    the existing real Brigadier oracle already covers a scoped subset. This
    probe checks actual score values *on the wire*, not just server internals.
    """
    result = []
    for line in lines:
        if ' error ' in f' {line} ' or 'disconnected' in line:
            raise AssertionError('Client/server protocol error: '+line)
        if ' score ' in line or ' display ' in line:
            result.append(line)
        if ' chat ' in line and 'commands.scoreboard.players.get.success' in line:
            who, rest = line.split(' chat ', 1)
            try:
                component = json.loads(rest)
                args = component['with']
                result.append(f"{who} query_score {args[1]}")
            except (KeyError, IndexError, TypeError, ValueError) as e:
                raise AssertionError(f'failed decoding /scoreboard feedback {line}: {e}') from e
    return result

def main():
    lock,jars,libraries=base.verify_artifacts()
    assert lock['minecraft']['version']=='26.3'
    hashes=base.source_hashes(SOURCE_PARTS)
    build=lab.LOCAL/NAME
    build.mkdir(parents=True,exist_ok=True)
    report=build/'report.json'
    report.unlink(missing_ok=True)
    classes=build/'classes'
    classes.mkdir(exist_ok=True)
    deps=os.pathsep.join(map(str,[jars['server'],*libraries]))
    oracle=lab.ROOT/'mods/horizon/src/test/java/dev/horizonlab/parity/LocatorBarClient.java'
    with (build/'compile-java.log').open('w') as log:
        subprocess.run(['javac','--release',str(lock['minecraft']['java_major']),'-proc:none','-nowarn',
                        '-sourcepath',str(lab.ROOT/'mods/horizon/src/test/java'),'-cp',deps,
                        '-d',str(classes),str(oracle)], cwd=lab.ROOT,check=True,stdout=log,stderr=subprocess.STDOUT,timeout=180)
    with (build/'compile-rust.log').open('w') as log:
        subprocess.run(['cargo','build','--locked','--release','-p','horizon-client','--bin','horizon-client'],
                       cwd=lab.ROOT,check=True,stdout=log,stderr=subprocess.STDOUT,timeout=900)
    meta=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1'],cwd=lab.ROOT,text=True))
    binary=Path(meta['target_directory'])/'release/horizon-client'
    vanilla=['java','-Xmx2g','-Duser.timezone=UTC','-cp',deps,'net.minecraft.server.Main','nogui']
    horizon=[binary,'--resources',lab.LOCAL/'horizon-client/26.3','--dedicated','--nogui']
    client=['java','-Xmx1g','-cp',str(classes)+os.pathsep+deps,'dev.horizonlab.parity.LocatorBarClient']
    original_script,original_seconds=loc.SCRIPT,loc.STEP_SECONDS
    try:
        loc.SCRIPT,loc.STEP_SECONDS=SCRIPT,2.0
        world=loc.generate_world(vanilla,build)
        observed_vanilla=loc.run_side('vanilla',vanilla,client,build,world)
        observed_horizon=loc.run_side('horizon',horizon,client,build,world)
    finally:
        loc.SCRIPT,loc.STEP_SECONDS=original_script,original_seconds
    a,b=filtered(observed_vanilla),filtered(observed_horizon)
    if a!=b:
        diff='\n'.join(difflib.unified_diff(a,b,'vanilla','horizon',n=2,lineterm=''))
        raise AssertionError(f'{len(a)} vanilla vs {len(b)} Horizon player packets mismatch:\n{diff[:13000]}')
    if len([line for line in a if ' score ' in line]) < 10:
        raise AssertionError('not enough player-observed chain/repeat score packets')
    if not any(' display SIDEBAR cbview' in line for line in a):
        raise AssertionError('missing original-codec sidebar display packet')
    if not any('Bob step=bob-joins-late display SIDEBAR cbview' in line for line in a):
        raise AssertionError('late joining Bob was not sent the active sidebar')
    base.ensure_sources_unchanged(SOURCE_PARTS,hashes)
    lab.write_json(report, {
        'schema':1,'minecraft':'26.3','manifest_sha1':lock['minecraft']['manifest_sha1'],
        'java':subprocess.check_output(['java','--version'],text=True).splitlines()[0],
        'platform':platform.platform(),'packets':len(a),'stages':len(SCRIPT),
        'source_hashes':hashes,
        'scope':'Two real servers and two original Java 26.3 codec players: conditional chain success/failure/skip/unconditional recovery; selectors with tag and distance; freeze and exact three repeating ticks; observed scoreboard and system-chat packets; not every command/permission scenario.',
    })
    print(f'PASS command-block-player: {len(a)} exact observed score/chat lines, {len(SCRIPT)} stages')

if __name__=='__main__':
    try:main()
    except (AssertionError,subprocess.CalledProcessError) as e:sys.exit(f'{NAME}: {e}')
