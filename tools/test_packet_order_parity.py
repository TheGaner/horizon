#!/usr/bin/env python3
"""Differential *wire ordering* matrix on Minecraft Java 26.3 and Horizon.

A single fresh vanilla world serves independent real dedicated servers. Two
clients use the *original Java 26.3 codec*, not a guessed protocol. Within
one connection TCP preserves order; across different connections there is no
global packet order. Compare observed packet streams per receiver and script
stage, retaining both strict-order and unordered event-set comparisons.

The original-codec driver sends valid packets, not physical GLFW keypresses.
The inventory click driver deliberately supplies an EMPTY client delta to test
server reconciliation (rather than mimicking vanilla UI prediction).

Run with source ../scripts/horizon_parity_env.sh then
  python3 tools/test_packet_order_parity.py
Outputs .local/packet-order-parity/report.json and both raw transcripts.
Any order mismatch fails the strict gate, even if all item counts match.
"""
import collections
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

NAME = 'packet-order-parity'
SOURCE_PARTS = [
    Path(__file__).resolve(),
    lab.ROOT / 'mods/horizon/src/test/java/dev/horizonlab/parity/LocatorBarClient.java',
    lab.ROOT / 'tools/test_locator_bar_parity.py',
    lab.ROOT / 'tools/test_dedicated_server_parity.py',
    lab.ROOT / 'crates/horizon-client/src/game/lan_actions.rs',
    lab.ROOT / 'crates/horizon-client/src/game/lan_crafting.rs',
    lab.ROOT / 'crates/horizon-client/src/lan/containers.rs',
    lab.ROOT / 'crates/horizon-client/src/lan/menu.rs',
    lab.ROOT / 'crates/horizon-client/src/lan/hub.rs',
    lab.ROOT / 'crates/horizon-client/src/game.rs',
    lab.ROOT / 'crates/horizon-client/src/entities.rs',
    lab.ROOT / 'crates/horizon-client/src/entity_nbt.rs',
    lab.ROOT / 'crates/horizon-client/src/game/remote.rs',
    lab.ROOT / 'crates/horizon-client/src/lan/entity.rs',
]

# The settle time is intentionally fixed for reproducibility, but packet
# timestamps and OS scheduling are not assumed equivalent between servers.
STEP_SECONDS = 1.0
SCRIPT = [
    ('alice-joins', ['difficulty peaceful'], ['join Alice', 'cmd Alice gamemode survival']),
    ('bob-joins', [], ['join Bob', 'cmd Bob gamemode survival']),
    ('setup-telemetry', [], [
        'state on', 'items on',
        'cmd Alice tp @s 0.5 -59 0.5',
        'cmd Bob tp @s 5.5 -59 0.5',
    ]),
]
FAMILIES = {}

def case(family, label, console=(), actions=(), *, clear_drops=True):
    assert label not in FAMILIES, label
    FAMILIES[label] = family
    # In vanilla the /item command can broadcast its new slot immediately,
    # while Horizon may coalesce it until the next tick. If we send the click
    # in the same stage, those *setup* slot notifications become apparent
    # content differences even when the action had identical effects. Keep
    # setup and the measured action in separate, settled script stages.
    actions = list(actions)
    first_action = next((i for i, a in enumerate(actions)
                         if a.startswith(('drop ', 'throw ', 'click '))), len(actions))
    prepare, execute = actions[:first_action], actions[first_action:]
    assert execute, label
    # Even a previous selection or menu cursor pickup must not leak into an
    # unrelated case. The outside click is setup, never compared as part of
    # the action under test.
    setup = ['select Alice 0', 'select Bob 0']
    if family == 'menu-click':
        setup.append('click Alice -999:0:PICKUP')
    setup.extend(prepare)
    SCRIPT.append((label+'-prepare',
                   (['kill @e[type=minecraft:item]'] if clear_drops else []) + list(console), setup))
    SCRIPT.append((label, [], execute))

def replace(player, slot, item, count):
    return f'cmd {player} item replace entity @s {slot} with minecraft:{item} {count}'

# Batch A: Q/Ctrl+Q serverbound player action, stack limits and empty hand.
for count in (1, 2, 8, 64):
    for mode in ('single', 'all'):
        case('hotbar-drop', f'hotbar-{mode}-stick-{count}', actions=[
            replace('Alice', 'hotbar.0', 'stick', count), f'drop Alice {mode}'])
for item, count, mode in [('stone',64,'single'), ('stone',64,'all'),
                          ('snowball',16,'single'), ('egg',16,'all'),
                          ('diamond_sword',1,'single'), ('diamond_sword',1,'all')]:
    case('hotbar-drop', f'hotbar-{mode}-{item}-{count}', actions=[
        replace('Alice','hotbar.0',item,count), f'drop Alice {mode}'])
for mode in ('single','all'):
    case('hotbar-empty', f'hotbar-empty-{mode}', actions=[
        replace('Alice','hotbar.0','air',1), f'drop Alice {mode}'])

# Batch B: inventory THROW, including a full stack, empty and different slots.
for count in (1, 2, 8, 64):
    for mode in ('single', 'all'):
        case('gui-throw', f'gui-{mode}-stick-{count}', actions=[
            replace('Alice','hotbar.0','stick',count), f'throw Alice {mode}:36'])
for mode in ('single', 'all'):
    case('gui-empty', f'gui-empty-{mode}', actions=[
        replace('Alice','hotbar.0','air',1), f'throw Alice {mode}:36'])
for slot, item_slot, item, count in [
    (37, 'hotbar.1', 'stick', 11),
    (44, 'hotbar.8', 'stone', 8),
    (9, 'inventory.0', 'stick', 4),
    (35, 'inventory.26', 'dirt', 3),
    (45, 'weapon.offhand', 'shield', 1),
    (5, 'armor.head', 'carved_pumpkin', 1),
]:
    for mode in ('single', 'all'):
        case('gui-slots', f'gui-{mode}-slot-{slot}', actions=[
            replace('Alice',item_slot,item,count), f'throw Alice {mode}:{slot}'])

# Batch C: exact hotbar selection and consecutive actions on a single TCP stream.
for selected in (1, 8, 0):
    case('selection', f'selected-hotbar-{selected}', actions=[
        f'select Alice {selected}', replace('Alice',f'hotbar.{selected}','stick',6), 'drop Alice single'])
case('rapid-actions','same-step-double-q', actions=[
    'select Alice 0', replace('Alice','hotbar.0','stick',9), 'drop Alice single', 'drop Alice single'])
case('rapid-actions','same-step-q-then-all', actions=[
    replace('Alice','hotbar.0','stick',9), 'drop Alice single', 'drop Alice all'])
case('rapid-actions','same-step-gui-double-throw', actions=[
    replace('Alice','hotbar.0','stick',9), 'throw Alice single:36', 'throw Alice single:36'])
case('rapid-actions','same-step-hotbar-then-gui', actions=[
    replace('Alice','hotbar.0','stick',9), 'drop Alice single', 'throw Alice all:36'])
case('rapid-actions','same-step-gui-then-hotbar', actions=[
    replace('Alice','hotbar.0','stick',9), 'throw Alice single:36', 'drop Alice all'])
case('rapid-actions','swap-selected-between-drops', actions=[
    replace('Alice','hotbar.0','stick',9), replace('Alice','hotbar.1','dirt',5),
    'select Alice 1', 'drop Alice single', 'select Alice 0', 'drop Alice all'])

# Batch D: per-receiver order when actions from two TCP connections interleave.
case('two-player','two-players-drop', actions=[
    replace('Alice','hotbar.0','stick',8), replace('Bob','hotbar.0','stone',5),
    'drop Alice single', 'drop Bob single'])
case('two-player','two-players-gui', actions=[
    replace('Alice','hotbar.0','stick',8), replace('Bob','hotbar.0','stone',5),
    'throw Alice single:36', 'throw Bob all:36'])

# Batch E: menu client reconciliation (deliberately empty predicted delta).
case('menu-click','pickup-left', actions=[replace('Alice','hotbar.0','stick',8), 'click Alice 36:0:PICKUP'])
case('menu-click','pickup-right', actions=[replace('Alice','hotbar.0','stick',8), 'click Alice 36:1:PICKUP'])
case('menu-click','pickup-outside-all', actions=[
    replace('Alice','hotbar.0','stick',8), 'click Alice 36:0:PICKUP', 'click Alice -999:0:PICKUP'])
case('menu-click','pickup-outside-one', actions=[
    replace('Alice','hotbar.0','stick',8), 'click Alice 36:0:PICKUP', 'click Alice -999:1:PICKUP'])
case('menu-click','quick-move', actions=[replace('Alice','hotbar.0','stick',8), 'click Alice 36:0:QUICK_MOVE'])
case('menu-click','swap-hotbar', actions=[
    replace('Alice','hotbar.0','stick',8), replace('Alice','hotbar.1','dirt',4), 'click Alice 36:1:SWAP'])
case('menu-click','pickup-empty-slot', actions=[
    replace('Alice','inventory.10','air',1), 'click Alice 19:0:PICKUP'])

SCRIPT.append(('done', [], ['items off', 'state off']))

# Observe only packet types whose fields are normalized by the original codec.
# Never compare packet order *between* Alice and Bob, only within their own
# independently-ordered inbound packet streams.
KINDS = ('slot ', 'inventory ', 'item-added', 'item-data ', 'item-removed')

def group_events(lines):
    groups = collections.defaultdict(list)
    errors = []
    for line in lines:
        if ' error ' in f' {line} ' or 'disconnected' in line or ' killed ' in f' {line} ':
            errors.append(line)
        who, label, *parts = line.split(' ',2)
        if len(parts) == 0:
            continue
        if not parts[0].startswith(tuple(KINDS)):
            continue
        if label.startswith('step='):
            label = label[5:]
        groups[(who,label)].append(parts[0])
    return groups, errors


def assess(vanilla, horizon, cases=None):
    left, left_errors = group_events(vanilla)
    right, right_errors = group_events(horizon)
    semantic = []
    ordering = []
    strict_cases = 0
    total_packets = 0
    selected=set(cases) if cases is not None else {key[1] for key in set(left)|set(right)}
    for who, label in sorted(((who, label) for who in ('Alice','Bob') for label in selected)
                             if cases is not None else set(left)|set(right)):
        if label not in FAMILIES:
            continue
        a = left.get((who,label), [])
        b = right.get((who,label), [])
        total_packets += len(a)
        if a == b:
            strict_cases += 1
            continue
        different = dict(player=who,case=label,family=FAMILIES[label],vanilla=a,horizon=b,
                         vanilla_count=len(a),horizon_count=len(b))
        if collections.Counter(a) == collections.Counter(b):
            different['kind']='ordering-only'
            ordering.append(different)
        else:
            different['kind']='content-or-count'
            different['diff']='\n'.join(difflib.unified_diff(a,b,'vanilla','horizon',lineterm=''))
            semantic.append(different)
    # An action with neither a spawn nor a slot response is NOT an automatic
    # success. Vanilla may intentionally reject some inputs, or a stateful
    # rate limit may suppress it; expose these as coverage gaps for review.
    expected_spawns={'hotbar-drop','gui-throw','gui-slots','selection',
                     'rapid-actions','two-player'}
    unobserved=[]
    for label, family in FAMILIES.items():
        if label not in selected or family not in expected_spawns:
            continue
        for who in ('Alice',):
            if not any(s=='item-added' for s in left.get((who,label),[])):
                unobserved.append(dict(player=who,case=label,family=family,side='vanilla'))
            if not any(s=='item-added' for s in right.get((who,label),[])):
                unobserved.append(dict(player=who,case=label,family=family,side='horizon'))
    return dict(
        cases=len(selected), receiver_case_streams=2*len(selected),
        observed_receiver_case_streams=len({k for k in set(left)|set(right) if k[1] in selected}),
        strict_equal_receiver_case_streams=strict_cases,
        observed_packets_vanilla=total_packets,
        ordering_only=ordering, content_or_count=semantic,
        unobserved_expected_spawns=unobserved,
        errors_vanilla=left_errors, errors_horizon=right_errors)


def main():
    lock,jars,libraries = base.verify_artifacts()
    assert lock['minecraft']['version']=='26.3'
    hashes=base.source_hashes(SOURCE_PARTS)
    build=lab.LOCAL/NAME
    build.mkdir(parents=True,exist_ok=True)
    (build/'report.json').unlink(missing_ok=True)
    classes=build/'classes'
    classes.mkdir(exist_ok=True)
    deps=os.pathsep.join(map(str,[jars['server'],*libraries]))
    oracle=lab.ROOT/'mods/horizon/src/test/java/dev/horizonlab/parity/LocatorBarClient.java'
    with (build/'compile-java.log').open('w') as log:
        subprocess.run(['javac','--release',str(lock['minecraft']['java_major']),'-proc:none',
                        '-nowarn','-sourcepath',str(lab.ROOT/'mods/horizon/src/test/java'),
                        '-cp',deps,'-d',str(classes),str(oracle)],cwd=lab.ROOT,check=True,
                       stdout=log,stderr=subprocess.STDOUT,timeout=180)
    with (build/'compile-rust.log').open('w') as log:
        subprocess.run(['cargo','build','--locked','--release','-p','horizon-client','--bin','horizon-client'],
                       cwd=lab.ROOT,check=True,stdout=log,stderr=subprocess.STDOUT,timeout=900)
    metadata=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1'],
                                               cwd=lab.ROOT,text=True))
    binary=Path(metadata['target_directory'])/'release/horizon-client'
    vanilla_server=['java','-Xmx2g','-Duser.timezone=UTC','-cp',deps,'net.minecraft.server.Main','nogui']
    horizon_server=[binary,'--resources',lab.LOCAL/'horizon-client/26.3','--dedicated','--nogui']
    client=['java','-Xmx1g','-cp',str(classes)+os.pathsep+deps,'dev.horizonlab.parity.LocatorBarClient']
    # Three separate server/client sessions limit stateful rate limits,
    # carried stacks, and entity tracker histories. Build once, generate
    # vanilla world once, and reuse the same world *copy* in each session.
    batches={
        'hotbar': {'hotbar-drop','hotbar-empty','selection','rapid-actions'},
        'gui': {'gui-throw','gui-empty','gui-slots'},
        'multiplayer-menus': {'two-player','menu-click'},
    }
    assert set().union(*batches.values())==set(FAMILIES.values())
    if len(sys.argv)>1:
        if len(sys.argv)!=3 or sys.argv[1]!='--batch' or sys.argv[2] not in batches:
            sys.exit(f'Usage: {sys.argv[0]} [--batch {"|".join(batches)}]')
        batches={sys.argv[2]:batches[sys.argv[2]]}
    original_script,original_seconds=loc.SCRIPT,loc.STEP_SECONDS
    a,b=[],[]
    batch_reports={}
    try:
        loc.STEP_SECONDS=STEP_SECONDS
        world=loc.generate_world(vanilla_server,build)
        for batch_name, families in batches.items():
            directory=build/batch_name
            directory.mkdir(exist_ok=True)
            selected={key for key,family in FAMILIES.items() if family in families}
            loc.SCRIPT=SCRIPT[:3]+[stage for stage in SCRIPT[3:-1]
                                  if stage[0] in selected or stage[0].removesuffix('-prepare') in selected]+SCRIPT[-1:]
            print(f'Running {batch_name}: {len(selected)} cases, {len(loc.SCRIPT)} stages',flush=True)
            left=loc.run_side('vanilla',vanilla_server,client,directory,world)
            right=loc.run_side('horizon',horizon_server,client,directory,world)
            a.extend(left)
            b.extend(right)
            batch_result=assess(left,right,cases=selected)
            batch_reports[batch_name]={
                'families':sorted(families), 'cases':len(selected),
                'stages':len(loc.SCRIPT),
                'ordering_only':len(batch_result['ordering_only']),
                'content_or_count':len(batch_result['content_or_count']),
                'missing_expected_spawns':sum(1 for row in batch_result['unobserved_expected_spawns'] if row['case'] in selected),
            }
            print(f"  {batch_name}: {batch_reports[batch_name]}",flush=True)
    finally:
        loc.SCRIPT,loc.STEP_SECONDS=original_script,original_seconds
    (build/'vanilla.txt').write_text('\n'.join(a)+'\n',encoding='utf-8')
    (build/'horizon.txt').write_text('\n'.join(b)+'\n',encoding='utf-8')
    covered={name for name, fam in FAMILIES.items() if fam in set().union(*batches.values())}
    result=assess(a,b,cases=covered)
    result['batches']=batch_reports
    result.update(schema=1,minecraft='26.3',families=dict(collections.Counter(FAMILIES.values())),
                  java=subprocess.check_output(['java','--version'],text=True).splitlines()[0],
                  platform=platform.platform(),source_hashes=hashes,
                  transcripts=dict(vanilla=lab.digest(build/'vanilla.txt','sha256'),
                                   horizon=lab.digest(build/'horizon.txt','sha256')),
                  interpretation=('Exact per-player server-to-client packet arrival ordering for slot, inventory, '
                                  'item spawn and item metadata. Not global order across TCP connections, '
                                  'GLFW keyboard timings, or all protocol packet types.'))
    lab.write_json(build/'report.json',result)
    base.ensure_sources_unchanged(SOURCE_PARTS,hashes)
    print(f"Packet-order matrix: {result['cases']} action cases, "
          f"{result['observed_packets_vanilla']} vanilla packets, "
          f"{result['strict_equal_receiver_case_streams']} exact receiver/case streams, "
          f"{len(result['ordering_only'])} ordering-only, "
          f"{len(result['content_or_count'])} content/count mismatches, "
          f"{len(result['unobserved_expected_spawns'])} unobserved expected spawns, "
          f"{len(result['errors_horizon'])+len(result['errors_vanilla'])} client errors")
    for issue in (result['content_or_count'] + result['ordering_only'])[:30]:
        print(f"  {issue['kind']} {issue['player']} {issue['case']}: "
              f"vanilla={issue['vanilla'][:7]} horizon={issue['horizon'][:7]}")
    if result['errors_horizon'] or result['errors_vanilla'] or result['content_or_count'] or result['ordering_only'] or result['unobserved_expected_spawns']:
        sys.exit('Strict packet-order parity differs; see .local/packet-order-parity/report.json')
    if result['observed_packets_vanilla'] < 12:
        sys.exit('Insufficient packet observations; missing instrumentation')
    print('PASS strict receiver-local packet-order parity')

if __name__=='__main__':
    try: main()
    except (AssertionError,subprocess.CalledProcessError) as exc:
        sys.exit(f'{NAME}: {exc}')
