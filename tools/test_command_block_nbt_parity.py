#!/usr/bin/env python3
"""Real Java 26.3 CommandBlockEntity NBT save/load vs Horizon CommandBlock.

The Java oracle calls actual saveWithFullMetadata/saveCustomOnly/loadStatic,
including ComponentSerialization for styled text. All game code remains in
ignored .local reference jars. Both independent outputs are compared in full.
"""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import random
import subprocess
try:
    from . import lab, test_rust_parity as base
except ImportError:
    import lab, test_rust_parity as base

ROOT = lab.ROOT
ORACLE = ROOT/'mods/horizon/src/test/java/dev/horizonlab/parity/VanillaCommandBlockNbtOracle.java'
HELPER = ORACLE.with_name('ParityData.java')
RUST = ROOT/'crates/horizon-client/examples/command_block_nbt_fixture.rs'
PRODUCTION = ROOT/'crates/horizon-client/src/command_block.rs'


def corpus(scenarios=512, seed=0x26B10026):
    static=[
        ('~',0,'~',True,'~',True,-1,False,False,False,False),
        ('say hi',42,'Captain',True,'Finished!',True,99,True,False,True,False),
        ('say hi',42,'Captain',False,'Hidden',False,99,False,True,False,False),
        ('Searge',-4,'~',True,'#itzlipofutzli',True,0,True,True,True,True),
        ('say hi',4,'@red:Captain',True,'@red:Finished!',True,100,True,False,True,False),
        ('say hi',4,'~',True,'@red:Error',True,-1,False,False,False,False),
        ('~',2147483647,'@red:Long',True,'@red:Warning',True,9223372036854775807,True,True,False,True),
        ('give',-2147483648,'~',False,'@red:Hidden',False,-9223372036854775808,True,False,True,True),
    ]
    for item in static:
        yield encode(item)
    rng=random.Random(seed)
    words=['~','Searge','say hello','tp 10 30','give apple','/help','hello world','☃ snow','test \\ path','☕ cafe']
    names=['~','Alpha','Captain','@red:Warning','@red:Error','名前','Player 42','Special \\ name']
    output=['~','OK','Finished!','@red:Error','@red:Warning','☃ snow','Line\\text','Bonjour','☕']
    counts=[0,1,-1,2147483647,-2147483648,42,1337]
    ticks=[-9223372036854775808,-1,0,1,2,42,36000,9223372036854775807]
    for _ in range(scenarios):
        yield encode((rng.choice(words),rng.choice(counts),rng.choice(names),
            rng.choice([True,False]),rng.choice(output),rng.choice([True,False]),
            rng.choice(ticks),rng.choice([True,False]),rng.choice([True,False]),
            rng.choice([True,False]),rng.choice([True,False])))


def encode(row):
    return '|'.join(str(int(x)).replace('1','true').replace('0','false') if isinstance(x,bool)
                    else str(x) for x in row)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scenarios',type=int,default=512)
    parser.add_argument('--seed',type=int,default=0x26B10026)
    args=parser.parse_args()
    if not 0 <= args.scenarios <= 100000:parser.error('scenarios must be 0..100000')
    build=lab.LOCAL/'command-block-nbt-parity'
    build.mkdir(parents=True,exist_ok=True)
    report=build/'report.json';report.unlink(missing_ok=True)
    lock,jars,libraries=base.verify_artifacts()
    lines=list(corpus(args.scenarios,args.seed))
    inp=build/'inputs.txt';inp.write_text('\n'.join(lines)+'\n')
    input_hash=lab.digest(inp,'sha256')
    sources=[ORACLE,HELPER,RUST,Path(__file__).resolve(),PRODUCTION,
             ROOT/'crates/horizon-client/src/scoreboard.rs',ROOT/'Cargo.lock']
    source_hash=base.source_hashes(sources)
    classes=build/'classes';classes.mkdir(exist_ok=True)
    cp=os.pathsep.join(map(str,[jars['server'],*libraries]))

    def run(stage,cmd):
        with (build/(stage+'.log')).open('w') as log:
            subprocess.run(list(map(str,cmd)),cwd=ROOT,check=True,
                           timeout=1200,stdout=log,stderr=subprocess.STDOUT)
    run('compile-java',['javac','--release',lock['minecraft']['java_major'],
         '-proc:none','-cp',cp,'-d',classes,HELPER,ORACLE])
    run('compile-rust',['cargo','build','--locked','--release','-p','horizon-client',
         '--example','command_block_nbt_fixture'])
    meta=json.loads(subprocess.check_output(['cargo','metadata','--no-deps',
                        '--format-version','1'],cwd=ROOT,text=True))
    binary=Path(meta['target_directory'])/'release/examples'/('command_block_nbt_fixture.exe' if os.name=='nt' else 'command_block_nbt_fixture')
    run('rust',[binary,inp,build/'rust.txt'])
    run('java',['java','-Xmx1G','-cp',os.pathsep.join([str(classes),cp]),
         'dev.horizonlab.parity.VanillaCommandBlockNbtOracle',inp,build/'java.txt'])
    count=base.compare(build/'java.txt',build/'rust.txt')
    base.ensure_sources_unchanged(sources,source_hash)
    if lab.digest(inp,'sha256')!=input_hash:raise AssertionError('fixture inputs changed')
    payload={
        'schema':1,'minecraft':lock['minecraft']['version'],'manifest_sha1':lock['minecraft']['manifest_sha1'],
        'java':subprocess.check_output(['java','-version'],stderr=subprocess.STDOUT,text=True).splitlines()[0],
        'scenarios':args.scenarios,'seed':args.seed,'matched_roundtrips':count,
        'operations':dict(Counter('styled' if '@red:' in line else 'plain' for line in lines)),
        'inputs_sha256':input_hash,'oracle_sha256':lab.digest(build/'java.txt','sha256'),
        'rust_sha256':lab.digest(build/'rust.txt','sha256'),'sources_sha256':source_hash,
        'scope':'Real 26.3 Java CommandBlockEntity saveCustomOnly/saveWithFullMetadata/loadStatic vs production Rust CommandBlock::save/load. Exact 10 selected typed NBT fields, plain/styled literal components, power/auto/condition, last execution, optional fields; not full actual chunk lifecycle or general arbitrary component codec tree.'
    }
    lab.write_json(report,payload)
    print(f'command-block NBT parity passed: {count:,} full saved-and-reloaded snapshots; {args.scenarios} random cases; report {report}',flush=True)

if __name__=='__main__':main()
