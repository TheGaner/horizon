#!/usr/bin/env python3
"""Actual 26.3 CommandBlock state/tick transitions vs Horizon Rust.

Uses a stub ServerLevel and simplified deduplicated one-block tick queue to
isolate the shipped methods, not an integrated world or command executor.
The input is deterministic and snapshots are compared byte for byte.
"""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import random
import subprocess
try:
    from . import lab,test_rust_parity as base
except ImportError:
    import lab,test_rust_parity as base

ROOT=lab.ROOT
ORACLE=ROOT/'mods/horizon/src/test/java/dev/horizonlab/parity/VanillaCommandBlockTickOracle.java'
HELPER=ORACLE.with_name('ParityData.java')
RUST=ROOT/'crates/horizon-client/examples/command_block_tick_fixture.rs'


def corpus(scenarios=512,seed=0x26C0B10C):
    rng=random.Random(seed)
    kinds=['impulse','repeat','chain']
    scripted=[
        ['reset impulse false false false 0','signal true','signal true','tick','signal false','tick','signal true','tick'],
        ['reset repeat false false false 0','auto true','tick','tick','auto false','tick','signal true','tick','signal false','tick'],
        ['reset impulse true false true 1','signal true','back true 0','tick','signal false','signal true','back true 5','tick'],
        ['reset impulse true false true 0','signal true','back true 5','tick','signal false','signal true','tick'],
        ['reset repeat true false true 1','mode impulse','signal true','tick','mode repeat','auto true','tick','conditional false','tick'],
        ['reset repeat false false false 0','signal true','auto true','signal false','mode impulse','tick','mode repeat','tick'],
        ['reset chain true true true 8','signal true','tick','auto false','mode repeat','tick','mode impulse','tick'],
        ['reset impulse false false false 0','success 2147483647','conditional true','signal true','tick','success -2147483648','tick'],
        ['reset impulse false false false 0 searge','signal true','tick','signal false','signal true','tick'],
        ['reset repeat true true true 2 searge','mode impulse','signal true','tick','mode repeat','tick','tick'],
        ['reset impulse false false false 0','chain true false false true','signal true','tick','signal false','signal true','tick'],
        ['reset impulse false false false 0 searge','chain true true false true','signal true','tick','signal false','signal true','tick'],
        ['reset impulse true false true 1 searge','chain true true false true','signal true','tick','back true 0','signal false','signal true','tick'],
        ['reset repeat false true false 0','chain true true false true','tick','tick','chain false false false false','tick'],
        ['reset impulse false false false 0 searge','path 4 0 0000 01010101 0','signal true','tick'],
        ['reset impulse false false false 0 searge','path 4 1 0000 01010101 0','signal true','tick'],
        ['reset impulse false false false 0 searge','path 4 2 0000 01010101 0','signal true','tick'],
        ['reset impulse false false false 0 searge','path 4 3 0000 01010101 0','signal true','tick'],
        ['reset impulse false false false 0 searge','path 4 8 0000 01010101 0','signal true','tick'],
        ['reset impulse false false false 0 searge','path 4 8 0111 01010101 0','signal true','tick'],
        ['reset impulse false false false 0 searge','path 4 8 0111 01010101 2','signal true','tick'],
        ['reset impulse false false false 0 searge','path 6 8 011111 010001010101 0','signal true','tick'],
        ['reset impulse false false false 0 searge','path 8 8 00000000 0101010101010101 5','signal true','tick'],
        ['reset impulse false false false 0','command noop','signal true','tick','signal false','signal true','tick'],
        ['reset repeat false false false 0','command noop','auto true','tick','tick','tick','auto false','tick'],
        ['reset impulse true false true 1','command noop','signal true','tick','signal false','command searge','signal true','tick'],
        ['reset impulse false false false 0','command noop','invoke','invoke','tick','invoke','invoke'],
        ['reset impulse false false false 0','command searge','invoke','invoke','tick','invoke'],
        ['reset impulse false false false 0','command empty','invoke','invoke','tick','invoke'],
        ['reset repeat false false false 0','command noop','invoke','auto true','tick','invoke','tick'],
    ]
    for scenario in scripted:
        yield from scenario
    for _ in range(scenarios):
        kind=rng.choice(kinds)
        cond=rng.choice([True,False]);auto=rng.choice([True,False]);behind=rng.choice([True,False])
        number=rng.choice([-2147483648,-1,0,1,2,2147483647])
        yield f'reset {kind} {str(cond).lower()} {str(auto).lower()} {str(behind).lower()} {number} {rng.choice(['empty']*3+['searge'])}'
        if rng.randrange(5)==0:yield 'command noop'
        for __ in range(rng.randrange(18,41)):
            choice=rng.randrange(100)
            if choice<31:yield f'signal {str(rng.choice([True,False])).lower()}'
            elif choice<49:yield 'tick'
            elif choice<62:yield f'auto {str(rng.choice([True,False])).lower()}'
            elif choice<76:yield f'mode {rng.choice(kinds)}'
            elif choice<85:yield f'conditional {str(rng.choice([True,False])).lower()}'
            elif choice<90:
                present=rng.choice([True,False]);n=rng.choice([-1,0,1,2,2147483647]);yield f'back {str(present).lower()} {n}'
            elif choice<95:
                vals=[rng.choice([True,False]) for _ in range(4)]
                yield 'chain '+' '.join(str(v).lower() for v in vals)
            elif choice<97:
                size=rng.randrange(1,9);max_len=rng.choice([0,1,2,3,size-1,size,size+1,8])
                conditional=''.join(rng.choices('01',k=size))
                activation=''.join(rng.choices('01',k=size*2))
                turning=rng.choice([0,0,0,*range(1,size+1)])
                yield f'path {size} {max_len} {conditional} {activation} {turning}'
            elif choice<98:yield 'command '+rng.choice(['empty','searge','noop'])
            elif choice<99:yield 'invoke'
            else:yield f'success {rng.choice([-2147483648,-1,0,1,2,2147483647])}'
        yield 'tick';yield 'tick'


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scenarios',type=int,default=512)
    parser.add_argument('--seed',type=int,default=0x26C0B10C)
    args=parser.parse_args()
    if not 0<=args.scenarios<=100000:parser.error('scenarios must be 0..100000')
    build=lab.LOCAL/'command-block-tick-parity'
    build.mkdir(parents=True,exist_ok=True)
    report=build/'report.json';report.unlink(missing_ok=True)
    lock,jars,libraries=base.verify_artifacts()
    ops=list(corpus(args.scenarios,args.seed));inp=build/'inputs.txt';inp.write_text('\n'.join(ops)+'\n')
    inputs_sha=lab.digest(inp,'sha256')
    sources=[ORACLE,HELPER,RUST,Path(__file__).resolve(),ROOT/'crates/horizon-client/src/command_block.rs',ROOT/'crates/horizon-client/Cargo.toml',ROOT/'Cargo.lock']
    source_sha=base.source_hashes(sources)
    classes=build/'classes';classes.mkdir(exist_ok=True)
    cp=os.pathsep.join(map(str,[jars['server'],*libraries]))
    def run(name,command):
        with (build/(name+'.log')).open('w') as log:
            subprocess.run(list(map(str,command)),cwd=ROOT,check=True,timeout=1200,stdout=log,stderr=subprocess.STDOUT)
    run('compile-java',['javac','--release',lock['minecraft']['java_major'],'-proc:none','-cp',cp,'-d',classes,HELPER,ORACLE])
    run('compile-rust',['cargo','build','--locked','--release','-p','horizon-client','--example','command_block_tick_fixture'])
    metadata=json.loads(subprocess.check_output(['cargo','metadata','--no-deps','--format-version','1'],cwd=ROOT,text=True))
    executable=Path(metadata['target_directory'])/'release/examples'/('command_block_tick_fixture.exe' if os.name=='nt' else 'command_block_tick_fixture')
    run('rust',[executable,inp,build/'rust.txt'])
    run('java',['java','-Xmx1G','-cp',os.pathsep.join([str(classes),cp]),'dev.horizonlab.parity.VanillaCommandBlockTickOracle',inp,build/'java.txt'])
    count=base.compare(build/'java.txt',build/'rust.txt')
    base.ensure_sources_unchanged(sources,source_sha)
    if lab.digest(inp,'sha256')!=inputs_sha:raise AssertionError('Input fixtures changed during validation')
    data={
        'schema':1,'minecraft':lock['minecraft']['version'],'manifest_sha1':lock['minecraft']['manifest_sha1'],
        'java':subprocess.check_output(['java','-version'],stderr=subprocess.STDOUT,text=True).splitlines()[0],
        'scenario_count':args.scenarios,'operations':dict(Counter(x.split()[0] for x in ops)),
        'matched_snapshots':count,'oracle_output_sha256':lab.digest(build/'java.txt','sha256'),
        'rust_output_sha256':lab.digest(build/'rust.txt','sha256'),
        'inputs_sha256':inputs_sha,'sources_sha256':source_sha,
        'scope':'Bytecode CommandBlock.neighborChanged/tick and CommandBlockEntity.setAutomatic/onModeSwitch/markConditionMet, with empty commands or actual BaseCommandBlock Searge special-command execution and output. One root block, optional predecessor, and 1..8 chain blocks with configurable max sequence length, conditional/active flags and directional breaks. Clock and pending-tick dedup are explicit test stubs, not full LevelTicks; real Brigadier execution, world NBT and network not covered. Nonempty noop uses a stub world where vanilla command dispatch is disabled, exercising actual lastExecution and success-count semantics without a game server. Explicit invoke operations compare repeated BaseCommandBlock.performCommand calls within one tick and record the boolean result.'}
    lab.write_json(report,data)
    print(f"command-block tick parity passed: {count:,} snapshots, {args.scenarios} randomized scenarios, 26.3 JVM oracle vs Rust; report {report}",flush=True)

if __name__=='__main__':main()
