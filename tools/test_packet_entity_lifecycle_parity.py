#!/usr/bin/env python3
"""Minecraft Java 26.3 original-codec per-entity lifetime packet-order parity.

Two real 26.3 codec connections to real vanilla/Horizon dedicated servers.
Validate spawn -> item stack metadata -> remove causality, reference identity,
and exact receiver-local ordering for isolated dropped items. Include repeated
single/all-stack/drop-through-GUI actions; no random entity IDs compared.
"""
import collections
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

NAME='packet-entity-lifecycle-parity'
SOURCE_PARTS=[Path(__file__).resolve(),
 lab.ROOT/'mods/horizon/src/test/java/dev/horizonlab/parity/LocatorBarClient.java',
 lab.ROOT/'tools/test_locator_bar_parity.py',
 lab.ROOT/'tools/test_dedicated_server_parity.py',
 lab.ROOT/'crates/horizon-client/src/game/lan.rs',
 lab.ROOT/'crates/horizon-client/src/game/lan_actions.rs',
 lab.ROOT/'crates/horizon-client/src/game/lan_crafting.rs',
 lab.ROOT/'crates/horizon-client/src/lan/entity.rs',
 lab.ROOT/'crates/horizon-client/src/lan/hub.rs',
 lab.ROOT/'crates/horizon-client/src/game.rs',
 lab.ROOT/'crates/horizon-client/src/entities.rs',
 lab.ROOT/'crates/horizon-client/src/entity_nbt.rs',
 lab.ROOT/'crates/horizon-client/src/game/remote.rs']
STEP_SECONDS=1.2
SCRIPT=[('alice-joins',[],['join Alice','cmd Alice gamemode survival']),
        ('bob-joins',[],['join Bob','cmd Bob gamemode survival']),
        ('setup',[],['state on','entitylife on','cmd Alice tp @s 0.5 -59 0.5',
                     'cmd Bob tp @s 5.5 -59 0.5'])]
CASES=[]
def add(label,item,count,mode='single',who='Alice',gui=False):
    CASES.append(label)
    SCRIPT.extend([
        (f'{label}-clear',['kill @e[type=minecraft:item]'],[]),
        (f'{label}-setup',[],[f'cmd {who} item replace entity @s hotbar.0 with minecraft:{item} {count}']),
        (f'{label}-spawn',[],[f'{"throw" if gui else "drop"} {who} {mode}']),
        (f'{label}-settle',[],[]),
        (f'{label}-remove',['kill @e[type=minecraft:item]'],[]),
    ])
for item, count, mode in [('stick',1,'single'),('stick',8,'single'),('stick',8,'all'),
                          ('stone',64,'single'),('stone',64,'all'),('snowball',16,'single'),
                          ('diamond_sword',1,'all')]:
    add(f'hotbar-{mode}-{item}-{count}',item,count,mode)
for item,count,mode in [('stick',1,'single'),('stick',8,'single'),('stick',8,'all'),
                        ('shield',1,'single')]:
    add(f'gui-{mode}-{item}-{count}',item,count,mode,gui=True)
add('bob-drops-stick-8','stick',8,who='Bob')
SCRIPT.append(('done',[],['entitylife off','state off']))


def parse(lines):
    groups=collections.defaultdict(list)
    errors=[]
    for line in lines:
        if ' error ' in f' {line} ' or 'disconnected' in line or ' killed ' in f' {line} ':
            errors.append(line)
        who,sep,rest=line.partition(' step=')
        if not sep:continue
        step,sep,event=rest.partition(' ')
        if event.startswith(('entity-spawn ','entity-meta ','entity-remove ','entity-take ')):
            groups[(who,step)].append(event)
    return groups,errors


def normalized_lifetimes(groups, who, label):
    events=[]
    for phase in ('spawn','settle','remove'):
        events.extend((phase,s) for s in groups.get((who,f'{label}-{phase}'),[]))
    # Entity ordinal numbers are generated independently for each receiver
    # and server, so map them to first-seen order within this one case.
    mapping={}
    canonical=[]
    errors=[]
    for phase,event in events:
        tokens=event.split(' ',2)
        op, raw=tokens[0],tokens[1]
        if op=='entity-spawn':
            if raw in mapping: errors.append('duplicate-spawn '+raw)
            mapping[raw]=f'#{len(mapping)+1}'
        elif raw not in mapping:
            errors.append(f'unknown-target {event}')
        canonical.append((phase,op,mapping.get(raw,'?'),tokens[2] if len(tokens)>2 else ''))
    for key in mapping:
        if not any(op=='entity-meta' and target==mapping[key] for _,op,target,_ in canonical):
            errors.append('missing-item-metadata '+key)
        if not any(op=='entity-remove' and target==mapping[key] for _,op,target,_ in canonical):
            errors.append('missing-entity-remove '+key)
    return canonical,errors


def evaluate(vanilla,horizon):
    lv,ev=parse(vanilla);lh,eh=parse(horizon)
    issues=[];exact=0;packets=0
    for label in CASES:
        for who in ('Alice','Bob'):
            a,ae=normalized_lifetimes(lv,who,label)
            b,be=normalized_lifetimes(lh,who,label)
            packets+=len(a)
            if ae or be or a!=b:
                issues.append(dict(case=label,player=who,vanilla=a,horizon=b,
                                   vanilla_errors=ae,horizon_errors=be))
            else:exact+=1
    return dict(cases=len(CASES),receiver_case_streams=2*len(CASES),
                exact_streams=exact,observed_vanilla_packets=packets,
                issues=issues,protocol_errors=ev+eh)


def main():
    lock,jars,libraries=base.verify_artifacts()
    assert lock['minecraft']['version']=='26.3'
    hashes=base.source_hashes(SOURCE_PARTS)
    build=lab.LOCAL/NAME
    build.mkdir(parents=True,exist_ok=True)
    (build/'report.json').unlink(missing_ok=True)
    classes=build/'classes';classes.mkdir(exist_ok=True)
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
    metadata=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1'],cwd=lab.ROOT,text=True))
    binary=Path(metadata['target_directory'])/'release/horizon-client'
    vanilla=['java','-Xmx2g','-Duser.timezone=UTC','-cp',deps,'net.minecraft.server.Main','nogui']
    horizon=[binary,'--resources',lab.LOCAL/'horizon-client/26.3','--dedicated','--nogui']
    client=['java','-Xmx1g','-cp',str(classes)+os.pathsep+deps,'dev.horizonlab.parity.LocatorBarClient']
    original_script,original_seconds=loc.SCRIPT,loc.STEP_SECONDS
    old_difficulty=loc.PROPERTIES.get('difficulty')
    try:
        # Superflat slimes can kill an idle headless player during a long
        # packet trace. Their random death drops are not part of this oracle.
        loc.PROPERTIES['difficulty']='peaceful'
        loc.SCRIPT,loc.STEP_SECONDS=SCRIPT,STEP_SECONDS
        world=loc.generate_world(vanilla,build)
        left=loc.run_side('vanilla',vanilla,client,build,world)
        right=loc.run_side('horizon',horizon,client,build,world)
    finally:
        if old_difficulty is None:
            loc.PROPERTIES.pop('difficulty',None)
        else:
            loc.PROPERTIES['difficulty']=old_difficulty
        loc.SCRIPT,loc.STEP_SECONDS=original_script,original_seconds
    result=evaluate(left,right)
    result.update(schema=1,minecraft='26.3',source_hashes=hashes,
                  java=subprocess.check_output(['java','--version'],text=True).splitlines()[0],
                  platform=platform.platform(),
                  transcript_sha256={'vanilla':lab.digest(build/'vanilla.txt','sha256'),
                                     'horizon':lab.digest(build/'horizon.txt','sha256')},
                  scope='Two separate Java 26.3 protocol-codec players, entity add/meta/remove and scoped item drops observed on both independent real servers; locally canonicalized entity IDs, no global packet order.')
    lab.write_json(build/'report.json',result)
    base.ensure_sources_unchanged(SOURCE_PARTS,hashes)
    print(f"Entity lifecycle: {len(CASES)} cases, {result['observed_vanilla_packets']} original entity packets, "
          f"{result['exact_streams']}/{result['receiver_case_streams']} exact receiver streams, "
          f"{len(result['issues'])} mismatches, {len(result['protocol_errors'])} errors")
    for issue in result['issues'][:14]:print('  DIFF',issue)
    if result['issues'] or result['protocol_errors'] or result['observed_vanilla_packets']<25:
        sys.exit('Entity wire/lifetime parity mismatches; see .local/packet-entity-lifecycle-parity/report.json')
    print('PASS exact entity lifetime packet order on both clients')

if __name__=='__main__':
    try:main()
    except (AssertionError,subprocess.CalledProcessError) as exc:sys.exit(f'{NAME}: {exc}')
