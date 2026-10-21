#!/usr/bin/env python3
"""Run real Java 26.3 ServerLevel command-block chunk/tick persistence lifecycle.

Live headless Mojang dedicated server, not an in-memory oracle:
- freeze the server tick loop; force-load a chunk far from spawn, create an
  automatic impulse command block, save a pending +1 block tick to .mca;
- stop the server, relaunch that world; verify the one pending tick dispatched
  and the scoreboard shows one execution, including rich LastOutput;
- unforce-load the chunk, wait until `execute if loaded` proves it is absent;
  reload the disk-backed chunk; a saved pending tick may execute again (not unconditional rearming);
- leave a fully saved real-world .mca NBT fixture for other Horizon readers.

Assertions are about original vanilla Java 26.3 world lifecycle only. The
comparison against Horizon's actual scheduler/client is a separate gate.
"""
import hashlib
import json
import shutil
from pathlib import Path
import re
import sys
import time
import uuid

try:
    from . import lab, test_rust_parity as base
    from .anvil_dump import region_chunks, plain
    from .test_save_roundtrip_parity import Vanilla, command_failed
except ImportError:
    import lab
    import test_rust_parity as base
    from anvil_dump import region_chunks, plain
    from test_save_roundtrip_parity import Vanilla, command_failed

NAME = 'command-block-server-lifecycle'
POS = (96,64,96)
CHUNK = (6,6)
WORLD_COORDS = '96 64 96'
BLOCK_COMMAND = 'scoreboard players add #probe cbhit 1'
PROPS = {'enable-command-block':'true', 'pause-when-empty-seconds':'-1',
         'max-tick-time':'-1', 'view-distance':'4', 'simulation-distance':'4'}


def check_command(server,command):
    output=server.run(command,timeout=90)
    if command_failed(output):
        raise AssertionError(f'command {command!r} failed: {output!r}')
    return output


def score(server, expected, timeout=45):
    end=time.monotonic()+timeout
    lines=[]
    while time.monotonic()<end:
        lines=server.run('scoreboard players get #probe cbhit',timeout=30)
        match=next((re.search(r'#probe has (-?\d+) \[cbhit\]',l) for l in lines
                    if '#probe has' in l),None)
        if match and match.group(1) is not None:
            value=int(match.group(1))
            if value==expected:return value
            if value>expected:raise AssertionError(f'scoreboard unexpectedly executed extra commands: {value}>{expected}')
        time.sleep(0.5)
    raise AssertionError(f'scoreboard did not reach {expected}: {lines!r}')


def persisted_block(folder,require_output):
    path=folder/'world/dimensions/minecraft/overworld/region/r.0.0.mca'
    chunks=region_chunks(path)
    chunk=chunks[CHUNK]
    blocks=[e for e in chunk['block_entities'] if e.get('id')=='minecraft:command_block'
            and (e.get('x'),e.get('y'),e.get('z'))==POS]
    if len(blocks)!=1:raise AssertionError(f'expected one persisted command block; got {len(blocks)}')
    block=blocks[0]
    if block.get('Command')!=BLOCK_COMMAND or plain(block.get('auto'),True)!=1:
        raise AssertionError(f'persisted command-block command/auto lost: {block!r}')
    if require_output:
        if plain(block.get('SuccessCount'),True)!=1:
            raise AssertionError(f'executed command success count lost: {block!r}')
        rich=block.get('LastOutput')
        if not isinstance(rich,dict) or not isinstance(rich.get('extra'),list):
            raise AssertionError(f'command output nested text component absent: {rich!r}')
    return chunk,block,path


def main():
    lock,jars,libs=base.verify_artifacts()
    assert lock['minecraft']['version']=='26.3'
    basepath=lab.LOCAL/NAME
    basepath.mkdir(parents=True,exist_ok=True)
    report=basepath/'report.json'
    report.unlink(missing_ok=True)
    source=Path(__file__).resolve()
    hashes=base.source_hashes([source,lab.ROOT/'tools/test_save_roundtrip_parity.py',
                              lab.ROOT/'tools/anvil_dump.py'])
    world=basepath/('world-'+uuid.uuid4().hex)
    world.mkdir()
    log=basepath/'session.log'
    details=[]
    server=None
    def note(stage,detail):
        details.append({'stage':stage,'details':detail})
        with log.open('a') as out:out.write(json.dumps(details[-1],sort_keys=True)+'\n')
        print(stage,detail,flush=True)
    try:
        server=Vanilla(world,extra=PROPS)
        check_command(server,'gamerule random_tick_speed 0')
        check_command(server,'scoreboard objectives add cbhit dummy')
        check_command(server,'forceload add 96 96')
        # Load the future chunk before freezing, so this test isolates the tick
        # from unrelated terrain generation; placing the block happens frozen.
        time.sleep(1)
        check_command(server,'tick freeze')
        check_command(server,'setblock 96 64 96 minecraft:command_block')
        check_command(server,
            'data merge block 96 64 96 {Command:"scoreboard players add #probe cbhit 1",auto:1b,TrackOutput:1b}')
        cmdstate=check_command(server,'data get block 96 64 96')
        if BLOCK_COMMAND not in ' '.join(cmdstate):raise AssertionError('command not stored in world')
        before=server.run('scoreboard players get #probe cbhit')
        if any('#probe has' in l for l in before):raise AssertionError('frozen command executed early')
        check_command(server,'save-all flush')
        chunk,block,path=persisted_block(world,False)
        ticks=[tick for tick in chunk.get('block_ticks',[])
               if tick.get('i')=='minecraft:command_block' and
                  (tick.get('x'),tick.get('y'),tick.get('z'))==POS]
        if len(ticks)!=1 or ticks[0].get('t')!=1 or ticks[0].get('p')!=0:
            raise AssertionError(f'expected a +1 normal-priority command block tick on disk: {ticks!r}')
        note('frozen_disk_save',{'pending_tick':plain(ticks[0],True),
                                 'region_bytes':path.stat().st_size})
        server.stop();server=None
        # Retain the exact frozen on-disk world before a pending tick can run.
        pending_snapshot=world/'pending_tick_snapshot'
        shutil.copytree(world/'world',pending_snapshot)
        (pending_snapshot/'session.lock').unlink(missing_ok=True)
        pending_region=pending_snapshot/'dimensions/minecraft/overworld/region/r.0.0.mca'
        pending_hash=hashlib.sha256(pending_region.read_bytes()).hexdigest()

        server=Vanilla(world,extra=PROPS)
        assert score(server,1)==1
        cmdstate=check_command(server,'data get block 96 64 96')
        if 'LastOutput' not in ' '.join(cmdstate) or 'SuccessCount: 1' not in ' '.join(cmdstate):
            raise AssertionError(f'command did not execute after restart: {cmdstate!r}')
        note('restart_dispatch',{'scoreboard_executions':1,'LastOutput':'present'})
        check_command(server,'forceload remove 96 96')
        # Direct `execute if loaded` proves the chunk is no longer loaded.
        deadline=time.monotonic()+60
        unloaded=False
        while time.monotonic()<deadline:
            response=server.run('execute if loaded 96 64 96 run say LIVE_CHUNK')
            if not any('LIVE_CHUNK' in l for l in response):
                unloaded=True
                break
            time.sleep(0.5)
        if not unloaded:raise AssertionError('chunk never unloaded after forceload remove')
        note('in_process_unload',{'confirmed_unloaded':True})
        check_command(server,'forceload add 96 96')
        deadline=time.monotonic()+60
        reloaded=[]
        while time.monotonic()<deadline:
            reloaded=server.run('execute if loaded 96 64 96 run say RELOADED_CHUNK')
            if any('RELOADED_CHUNK' in l for l in reloaded):
                break
            time.sleep(0.5)
        else:
            raise AssertionError(f'chunk did not reload after forceload add: {reloaded!r}')
        assert score(server,2)==2
        note('in_process_reload',{'scoreboard_executions':2,
                                  'saved_pending_tick_replayed_on_loading':True})
        # A command result alone does not always dirty the owning chunk in
        # vanilla; mark the neighbour as changed before asserting that
        # save-all flush persisted the updated LastOutput/SuccessCount.
        check_command(server,'setblock 97 64 96 minecraft:stone')
        check_command(server,'save-all flush')
        server.stop();server=None
        chunk,block,path=persisted_block(world,True)
        if plain(block.get('SuccessCount'),True)!=1:
            raise AssertionError('last command success count did not persist')
        out=block['LastOutput']
        if 'commands.scoreboard.players.add.success.single' not in str(out):
            raise AssertionError(f'vanilla nested output changed: {out!r}')
        note('saved_after_rearm',{'LastOutput_contains_translation':True,
                                  'success_count':1,'region_bytes':path.stat().st_size})
        base.ensure_sources_unchanged([source,lab.ROOT/'tools/test_save_roundtrip_parity.py',
                                       lab.ROOT/'tools/anvil_dump.py'],hashes)
        lab.write_json(report,{
            'schema':1,'minecraft':lock['minecraft']['version'],'manifest_sha1':lock['minecraft']['manifest_sha1'],
            'steps':details,'region_sha256':hashlib.sha256(path.read_bytes()).hexdigest(),
            'world_directory':str(world),'pending_tick_world_directory':str(pending_snapshot),
            'pending_tick_region_sha256':pending_hash,'source_hashes':hashes,
            'scope':'Real official Minecraft Java 26.3 headless ServerLevel freeze/save/stop/restart and in-process chunk unload/reload. Horizon server behavior unverified.'})
        print('PASS official vanilla Java 26.3 live ServerLevel command-block disk scheduler/restart/reload',flush=True)
    finally:
        if server is not None:server.stop()

if __name__=='__main__':main()
