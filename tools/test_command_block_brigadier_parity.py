#!/usr/bin/env python3
"""Real Java 26.3 vs Horizon: nested execute, scoreboard and live automatic NBT activation.

Runs independently copied world saves on both dedicated servers, not synthetic
command evaluators. Captures scoreboard results and checks command-block
activation only when `/data merge block` changes `auto` from false to true.
"""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import time
import uuid

try:
    from . import lab, test_rust_parity as base
    from .test_dedicated_server_parity import Server, free_port, write_server_dir
    from .test_command_block_horizon_server_parity import reply, get_worlds
except ImportError:
    import lab, test_rust_parity as base
    from test_dedicated_server_parity import Server, free_port, write_server_dir
    from test_command_block_horizon_server_parity import reply, get_worlds

ROOT=lab.ROOT
NAME='command-block-brigadier'
PARTS=[Path(__file__).resolve(), ROOT/'tools/test_command_block_horizon_server_parity.py',
       ROOT/'tools/test_dedicated_server_parity.py',ROOT/'crates/horizon-client/src/command_block.rs',
       ROOT/'crates/horizon-client/src/game/command_blocks.rs',
       ROOT/'crates/horizon-client/src/game/commands/data.rs',
       ROOT/'crates/horizon-client/src/game/commands.rs']


def main():
    lock,_,_=base.verify_artifacts()
    assert lock['minecraft']['version']=='26.3'
    world,_=get_worlds()
    hashes=base.source_hashes(PARTS)
    dest=lab.LOCAL/NAME
    dest.mkdir(parents=True,exist_ok=True)
    report=dest/'report.json'
    report.unlink(missing_ok=True)
    run=dest/('run-'+uuid.uuid4().hex)
    run.mkdir()
    subprocess.run(['cargo','build','--locked','--release','-p','horizon-client','--bin','horizon-client'],
                   check=True,cwd=ROOT,stdout=(dest/'build.log').open('w'),stderr=subprocess.STDOUT,timeout=900)
    meta=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1'],cwd=ROOT,text=True))
    binary=Path(meta['target_directory'])/'release/horizon-client'
    executables={
      'vanilla':['java','-Xmx2g','-jar',lab.LOCAL/'minecraft/26.3/jars/server.jar','--nogui'],
      'horizon':[binary,'--resources',lab.LOCAL/'horizon-client/26.3','--dedicated','--nogui'],
    }
    steps=[
      'scoreboard objectives add expr dummy',
      'scoreboard players set #a expr 10',
      'scoreboard players set #b expr 3',
      'execute store result score #result expr run scoreboard players operation #a expr += #b expr',
      'execute if score #a expr matches 13 run scoreboard players add #check expr 1',
      'execute unless score #b expr matches 4 run scoreboard players add #check expr 2',
      'execute positioned 96 64 96 if block 96 64 96 minecraft:command_block run scoreboard players add #check expr 5',
      'execute store result score #out expr run data get block 96 64 96 SuccessCount',
      'execute if data block 96 64 96 {auto:1b} run scoreboard players add #check expr 7',
      'scoreboard players operation #copy expr = #check expr',
      'setblock 99 64 96 minecraft:command_block',
      'data merge block 99 64 96 {Command:"execute if score #check expr matches 15 run scoreboard players add #cb expr 1",auto:1b,TrackOutput:1b}',
    ]
    expected={'#a':13,'#b':3,'#result':13,'#check':15,'#out':1,'#copy':15,'#cb':1}
    results={}
    for name,exe in executables.items():
        folder=run/name
        write_server_dir(folder,free_port(),world)
        with (folder/'server.properties').open('a') as f:
            f.write('enable-command-block=true\npause-when-empty-seconds=-1\n')
        server=Server(exe,folder,name)
        serial=0
        def ask(c):
            nonlocal serial
            serial+=1
            return '\n'.join(reply(server,c,serial))
        try:
            server.wait_for(r'Done \(',180)
            for cmd in steps:
                response=ask(cmd)
                if 'Unknown command' in response or 'Unknown or incomplete command' in response:
                    raise AssertionError(f'{name}: command not recognized: {cmd} -> {response}')
            # The automatic impulse is scheduled for one game tick. Give
            # ample time for asynchronously loaded chunks to become ticking.
            deadline=time.monotonic()+20
            while time.monotonic()<deadline:
                if '#cb has 1 [expr]' in ask('scoreboard players get #cb expr'):
                    break
                time.sleep(.3)
            else:
                raise AssertionError(f'{name}: failed to execute auto block after data merge')
            values={}
            for holder,want in expected.items():
                response=ask(f'scoreboard players get {holder} expr')
                if f'{holder} has {want} [expr]' not in response:
                    raise AssertionError(f'{name}: {holder} expected {want}, got {response}')
                values[holder]=want
            block=ask('data get block 99 64 96')
            if 'SuccessCount: 1' not in block or 'LastOutput' not in block or 'hover_event' not in block:
                raise AssertionError(f'{name}: automatic NBT block output differs: {block[:1000]}')
            # Changing the command alone while it remains automatic must not
            # re-run an impulse block (old auto=true -> new auto=true).
            ask('data merge block 99 64 96 {Command:"scoreboard players add #cb expr 100"}')
            time.sleep(2)
            response=ask('scoreboard players get #cb expr')
            if '#cb has 1 [expr]' not in response:
                raise AssertionError(f'{name}: auto impulse incorrectly rearmed on command-only edit: {response}')
        finally:
            server.stop()
        results[name]={'scores':values,'auto_data_merge_executions':1,'command_only_edit_reexecutions':0,
                       'nested_hover_output':True}
        print(name,'PASS nested execute/scoreboard/store/data and live auto false→true NBT transition',flush=True)
    assert results['vanilla']==results['horizon']
    base.ensure_sources_unchanged(PARTS,hashes)
    lab.write_json(report,{'schema':1,'minecraft':'26.3','manifest_sha1':lock['minecraft']['manifest_sha1'],
        'world':str(world),'results':results,'source_hashes':hashes,
        'scope':'Actual dedicated vanilla and Horizon 26.3, nested execute/store/scoreboard and live auto activation via data merge. Not exhaustive Brigadier or packet parity.'})
    print('PASS command-block-brigadier both real dedicated servers match',flush=True)

if __name__=='__main__':main()
