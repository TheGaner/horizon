#!/usr/bin/env python3
"""Actual 26.3 objective/score/display/reset packet ordering, two real servers."""
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

NAME='packet-scoreboard-order-parity'
SOURCE_PARTS=[Path(__file__).resolve(),
 lab.ROOT/'mods/horizon/src/test/java/dev/horizonlab/parity/LocatorBarClient.java',
 lab.ROOT/'tools/test_locator_bar_parity.py',
 lab.ROOT/'tools/test_dedicated_server_parity.py',
 lab.ROOT/'crates/horizon-client/src/game/scoreboard.rs',
 lab.ROOT/'crates/horizon-client/src/scoreboard.rs',
 lab.ROOT/'crates/horizon-client/src/game/commands.rs',
 lab.ROOT/'crates/horizon-client/src/game/commands/data.rs',
 lab.ROOT/'crates/horizon-client/src/game/lan.rs']
STEP_SECONDS=1.5
SCRIPT=[
 ('alice-joins',[],['join Alice','cmd Alice gamemode survival']),
 ('observe',[],['state on','objectives on']),
 ('create-hidden',['scoreboard objectives add wire dummy',
                   'scoreboard players set Alice wire 2'],[]),
 ('activate-sidebar',['scoreboard objectives setdisplay sidebar wire'],[]),
 ('score-add',['scoreboard players add Alice wire 5'],[]),
 ('score-bob-before-join',['scoreboard players set Bob wire 9'],[]),
 ('bob-joins-late',[],['join Bob','cmd Bob gamemode survival']),
 ('score-bob-add',['scoreboard players add Bob wire 1'],[]),
 ('score-reset',['scoreboard players reset Alice wire'],[]),
 ('score-new',['scoreboard players set Alice wire 3'],[]),
 ('display-list',['scoreboard objectives setdisplay list wire'],[]),
 ('display-below',['scoreboard objectives setdisplay below_name wire'],[]),
 ('display-clear-sidebar',['scoreboard objectives setdisplay sidebar'],[]),
 ('score-remove-bob',['scoreboard players reset Bob wire'],[]),
 ('objective-remove',['scoreboard objectives remove wire'],[]),
 ('second-objective',['scoreboard objectives add next dummy',
                      'scoreboard players set Alice next 11'],[]),
 ('second-display',['scoreboard objectives setdisplay sidebar next'],[]),
 ('second-score',['scoreboard players add Alice next 2'],[]),
 ('second-remove',['scoreboard objectives remove next'],[]),
 # Two simultaneously tracked objectives expose whether a full-holder reset
 # emits one null-objective packet, not one per objective. A scoped reset
 # while another score exists must instead name that one objective.
 ('two-objectives-create',['scoreboard objectives add leftobj dummy',
                           'scoreboard objectives add rightobj dummy'],[]),
 ('two-objectives-display',['scoreboard objectives setdisplay sidebar leftobj',
                            'scoreboard objectives setdisplay list rightobj'],[]),
 ('two-objectives-score',['scoreboard players set Alice leftobj 13',
                          'scoreboard players set Alice rightobj 21'],[]),
 ('two-objectives-reset-all',['scoreboard players reset Alice'],[]),
 ('two-objectives-score-again',['scoreboard players set Alice leftobj 7',
                                'scoreboard players set Alice rightobj 9'],[]),
 ('two-objectives-reset-one',['scoreboard players reset Alice leftobj'],[]),
 ('two-objectives-reset-last',['scoreboard players reset Alice rightobj'],[]),
 ('two-objectives-remove',['scoreboard objectives remove leftobj',
                           'scoreboard objectives remove rightobj'],[]),
 ('done',[],['objectives off','state off'])
]
KINDS=('objective ','reset-score ','score ','display ')

def events(lines):
    by={'Alice':[],'Bob':[]}
    problems=[]
    for line in lines:
        if ' error ' in f' {line} ' or 'disconnected' in line:
            problems.append(line)
        who,sep,rest=line.partition(' step=')
        if not sep or who not in by:continue
        stage,sep,event=rest.partition(' ')
        if event.startswith(KINDS):
            by[who].append((stage,event))
    return by,problems


def assess(vanilla,horizon):
    v,ve=events(vanilla)
    h,he=events(horizon)
    diffs={}
    for who in ('Alice','Bob'):
        if v[who]!=h[who]:
            diffs[who]='\n'.join(difflib.unified_diff(
                [f'{s} {p}' for s,p in v[who]],
                [f'{s} {p}' for s,p in h[who]],
                f'vanilla/{who}',f'horizon/{who}',lineterm=''))
    return dict(vanilla_packets={who:len(v[who]) for who in v},
                horizon_packets={who:len(h[who]) for who in h},
                differences=diffs,protocol_errors=ve+he,
                vanilla_sequence=v,horizon_sequence=h)


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
    saved_script,saved_seconds=loc.SCRIPT,loc.STEP_SECONDS
    try:
        loc.SCRIPT,loc.STEP_SECONDS=SCRIPT,STEP_SECONDS
        world=loc.generate_world(vanilla,build)
        left=loc.run_side('vanilla',vanilla,client,build,world)
        right=loc.run_side('horizon',horizon,client,build,world)
    finally:
        loc.SCRIPT,loc.STEP_SECONDS=saved_script,saved_seconds
    result=assess(left,right)
    base.ensure_sources_unchanged(SOURCE_PARTS,hashes)
    lab.write_json(build/'report.json',dict(schema=1,minecraft='26.3',
        source_hashes=hashes,steps=len(SCRIPT),result=result,
        java=subprocess.check_output(['java','--version'],text=True).splitlines()[0],
        platform=platform.platform(),
        transcript_sha256={'vanilla':lab.digest(build/'vanilla.txt','sha256'),
                           'horizon':lab.digest(build/'horizon.txt','sha256')}))
    print(f"Scoreboard ordering: {len(SCRIPT)} stages, vanilla packets={result['vanilla_packets']}, "
          f"differences={len(result['differences'])}, errors={len(result['protocol_errors'])}")
    for who,diff in result['differences'].items():print('DIFF '+who+'\n'+diff[:6000])
    if result['protocol_errors'] or result['differences']:
        sys.exit('Scoreboard packet ordering mismatches; see .local/packet-scoreboard-order-parity/report.json')
    v=result['vanilla_sequence']
    if not any(step=='bob-joins-late' and event.startswith('objective 0 ') for step,event in v['Bob']):
        sys.exit('Late joiner objective snapshot not observed')
    if not any(event.startswith('reset-score ') for event in map(lambda t:t[1],v['Alice'])):
        sys.exit('Expected score-reset packet not observed')
    print('PASS strict objective/score/display lifecycle order')

if __name__=='__main__':
    try:main()
    except (AssertionError,subprocess.CalledProcessError) as e:sys.exit(f'{NAME}: {e}')
