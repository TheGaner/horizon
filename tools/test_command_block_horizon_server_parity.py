#!/usr/bin/env python3
"""Live Java 26.3 vs Horizon dedicated-server command block restart parity.

Both servers open *independent copies* of the same genuine vanilla 26.3 world
from test_command_block_server_lifecycle_parity. The saved command block has
nested translated LastOutput, success count one, `auto:1`, and **no pending
block tick**; the scoreboard already reads two. Also retain the actual *frozen* vanilla
world whose chunk has a pending +1 command tick and no score. Verify both
servers execute that saved tick once upon cold restart. For the already
executed fixture, verify both leave the scoreboard at two after genuine
forceload remove/unload/add/reload. Save and check persistent tickets.

This is a scoped running dedicated-server equality oracle. It does not test
all command executions, arbitrary chunk schedules or multiplayer clients.
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
    from . import test_command_block_server_lifecycle_parity as source
    from .anvil_dump import region_chunks, read_dat, plain
    from .test_dedicated_server_parity import Server, free_port, write_server_dir
except ImportError:
    import lab, test_rust_parity as base
    import test_command_block_server_lifecycle_parity as source
    from anvil_dump import region_chunks, read_dat, plain
    from test_dedicated_server_parity import Server, free_port, write_server_dir

ROOT=lab.ROOT
NAME='command-block-horizon-server'
PARTS=[Path(__file__).resolve(),Path(source.__file__).resolve(),
       ROOT/'tools/test_dedicated_server_parity.py',
       ROOT/'crates/horizon-client/src/game.rs',
       ROOT/'crates/horizon-client/src/save.rs',
       ROOT/'crates/horizon-client/src/command_block.rs']


def reply(server,command,counter,timeout=40):
    marker=f'CBPARITY_{counter:04d}'
    start=len(server.lines)
    server.console(command)
    server.console(f'say {marker}')
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        new=server.lines[start:]
        mark=next((i for i,s in enumerate(new) if marker in s),None)
        if mark is not None:return new[:mark]
        if server.process.poll() is not None:
            raise AssertionError(f'server crashed on {command}: {server.lines[-12:]}')
        time.sleep(.1)
    raise AssertionError(f'console timed out on {command}: {server.lines[start:][-8:]}')


def get_worlds():
    report=lab.LOCAL/'command-block-server-lifecycle/report.json'
    def usable():
        if not report.is_file():return False
        data=json.loads(report.read_text())
        return (Path(data['world_directory']).joinpath('world').is_dir()
                and Path(data.get('pending_tick_world_directory', '')).is_dir())
    if not usable():
        source.main() # Creates official saved before/after vanilla 26.3 worlds.
    data=json.loads(report.read_text())
    if data['minecraft'] != '26.3' or [s['stage'] for s in data['steps']] != [
         'frozen_disk_save','restart_dispatch','in_process_unload',
         'in_process_reload','saved_after_rearm']:
        raise AssertionError('reusable source fixture is not the complete pinned live 26.3 scenario')
    world=Path(data['world_directory'])/'world'
    saved=region_chunks(world/'dimensions/minecraft/overworld/region/r.0.0.mca')[(6,6)]
    assert not any(e.get('i')=='minecraft:command_block' and e.get('x')==96
                   for e in saved.get('block_ticks',[])), 'expected no pending tick after execution'
    pending=Path(data['pending_tick_world_directory'])
    frozen=region_chunks(pending/'dimensions/minecraft/overworld/region/r.0.0.mca')[(6,6)]
    assert any(e.get('i')=='minecraft:command_block' and e.get('x')==96
               and e.get('y')==64 and e.get('z')==96 and e.get('t')==1
               for e in frozen.get('block_ticks',[])), 'expected +1 tick in frozen vanilla snapshot'
    actual_hash=hashlib.sha256((pending/'dimensions/minecraft/overworld/region/r.0.0.mca').read_bytes()).hexdigest()
    assert actual_hash==data['pending_tick_region_sha256'], 'frozen vanilla snapshot changed'
    return world,pending


def ticket_positions(world):
    path=world/'dimensions/minecraft/overworld/data/minecraft/chunk_tickets.dat'
    data=plain(read_dat(path),True)
    return sorted(tuple(t['chunk_pos']) for t in data['data']['tickets']
                  if t.get('type')=='minecraft:forced')


def main():
    lock,jars,libs=base.verify_artifacts()
    if lock['minecraft']['version']!='26.3':raise AssertionError('pinned Minecraft mismatch')
    source_world,pending_world=get_worlds()
    hashes=base.source_hashes(PARTS)
    build=lab.LOCAL/NAME
    build.mkdir(parents=True,exist_ok=True)
    report=build/'report.json'
    report.unlink(missing_ok=True)
    run=build/('run-'+uuid.uuid4().hex)
    run.mkdir()
    expected=(6,6)
    if expected not in ticket_positions(source_world):
        raise AssertionError('original vanilla source fixture lacks persistent force-loaded chunk')
    subprocess.run(['cargo','build','--locked','--release','-p','horizon-client',
                    '--bin','horizon-client'],check=True,cwd=ROOT,
                    stdout=(build/'build.log').open('w'),stderr=subprocess.STDOUT,timeout=900)
    metadata=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1'],cwd=ROOT,text=True))
    binary=Path(metadata['target_directory'])/'release/horizon-client'
    commands={
        'vanilla':['java','-Xmx2g','-jar',lab.LOCAL/'minecraft/26.3/jars/server.jar','--nogui'],
        'horizon':[binary,'--resources',lab.LOCAL/'horizon-client/26.3','--dedicated','--nogui'],
    }
    results={}
    for name,cmd in commands.items():
        folder=run/name
        write_server_dir(folder,free_port(),source_world)
        with (folder/'server.properties').open('a') as f:
            f.write('enable-command-block=true\npause-when-empty-seconds=-1\n')
        server=Server(cmd,folder,name)
        n=0
        def ask(cmd):
            nonlocal n
            n+=1
            return '\n'.join(reply(server,cmd,n))
        try:
            server.wait_for(r'Done \(',180)
            scoreboard=ask('scoreboard players get #probe cbhit')
            if '#probe has 2 [cbhit]' not in scoreboard:
                raise AssertionError(f'{name}: saved scoreboard failed or spurious execution: {scoreboard}')
            original=ask('forceload query 96 96')
            if 'is marked for force loading' not in original:
                raise AssertionError(f'{name}: did not restore saved vanilla force-loaded chunk: {original}')
            block=ask('data get block 96 64 96')
            if 'scoreboard players add #probe cbhit 1' not in block or 'LastOutput' not in block:
                raise AssertionError(f'{name}: block NBT and rich LastOutput not restored: {block[:1200]}')
            if 'hover_event' not in block or 'SuccessCount: 1' not in block:
                raise AssertionError(f'{name}: actual translated hover text lost: {block[:1200]}')
            ask('forceload remove 96 96')
            expiry=time.monotonic()+65
            while time.monotonic()<expiry:
                result=ask('execute if loaded 96 64 96 run say CHECK_LOADED')
                if 'CHECK_LOADED' not in result:break
                time.sleep(.5)
            else:raise AssertionError(f'{name}: chunk never unloaded')
            ask('forceload add 96 96')
            expiry=time.monotonic()+65
            while time.monotonic()<expiry:
                result=ask('execute if loaded 96 64 96 run say CHECK_RELOADED')
                if 'CHECK_RELOADED' in result:break
                time.sleep(.5)
            else:raise AssertionError(f'{name}: chunk never reloaded')
            time.sleep(2)
            result=ask('scoreboard players get #probe cbhit')
            if '#probe has 2 [cbhit]' not in result:
                raise AssertionError(f'{name}: wrongly re-executed auto impulse without saved pending tick: {result}')
            ask('save-all flush')
        finally:
            server.stop()
        after=ticket_positions(folder/'world')
        if expected not in after:
            raise AssertionError(f'{name}: save removed a persistent forced chunk ticket: {after}')
        results[name]={'initial_score':2,'after_unload_reload_score':2,'forced_chunk':list(expected),
                       'nested_hover_event':'preserved','forced_ticket_persisted':True}
        print(name,'PASS real saved chunk force-load/reload, scoreboard=2, complex LastOutput preserved',flush=True)
    # Independent cold-restart dispatch case: this file was saved with one
    # pending scheduled tick while vanilla's tick loop was frozen. No score
    # exists in the stored world yet. Both dedicated servers must run it once.
    for name,cmd in commands.items():
        folder=run/(name+'-pending')
        write_server_dir(folder,free_port(),pending_world)
        with (folder/'server.properties').open('a') as f:
            f.write('enable-command-block=true\npause-when-empty-seconds=-1\n')
        server=Server(cmd,folder,name+'-pending')
        n=0
        def ask(line):
            nonlocal n
            n+=1
            return '\n'.join(reply(server,line,n))
        try:
            server.wait_for(r'Done \(',180)
            end=time.monotonic()+90
            while time.monotonic()<end:
                result=ask('scoreboard players get #probe cbhit')
                if '#probe has 1 [cbhit]' in result:
                    break
                if re.search(r'#probe has ([2-9]|[1-9]\d+) \[cbhit\]',result):
                    raise AssertionError(f'{name}: duplicated pending tick dispatch: {result}')
                time.sleep(.5)
            else:
                raise AssertionError(f'{name}: never executed the frozen saved +1 tick: {result}')
            if 'is marked for force loading' not in ask('forceload query 96 96'):
                raise AssertionError(f'{name}: pending-world forced ticket not restored')
            block=ask('data get block 96 64 96')
            if 'SuccessCount: 1' not in block or 'LastOutput' not in block:
                raise AssertionError(f'{name}: pending tick did not update actual block entity: {block[:900]}')
            time.sleep(2)
            after=ask('scoreboard players get #probe cbhit')
            if '#probe has 1 [cbhit]' not in after:
                raise AssertionError(f'{name}: saved tick executed more than once: {after}')
            ask('save-all flush')
        finally:
            server.stop()
        if (6,6) not in ticket_positions(folder/'world'):
            raise AssertionError(f'{name}: lost forced chunk after pending tick dispatch')
        results[name]['frozen_pending_tick_cold_restart_score']=1
        print(name,'PASS real frozen +1 saved command-block tick executed once after restart',flush=True)
    base.ensure_sources_unchanged(PARTS,hashes)
    record={'schema':1,'minecraft':'26.3','manifest_sha1':lock['minecraft']['manifest_sha1'],
            'source_world':str(source_world),'pending_tick_world':str(pending_world),'source_region_sha256':hashlib.sha256(
                (source_world/'dimensions/minecraft/overworld/region/r.0.0.mca').read_bytes()).hexdigest(),
            'sides':results,'source_hashes':hashes,
            'scope':'Actual dedicated vanilla Java 26.3 and Horizon run two identical real saved worlds: a frozen +1 pending command tick executes once on cold restart, and an already executed auto command block does not execute without a pending tick after unload/reload; forced tickets and rich output retained. No arbitrary command or client packets.'}
    lab.write_json(report,record)
    print('PASS running dedicated Horizon vs vanilla frozen pending tick and already-executed unload/reload parity',flush=True)

if __name__=='__main__':main()
