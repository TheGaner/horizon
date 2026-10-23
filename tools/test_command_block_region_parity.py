#!/usr/bin/env python3
"""Real Horizon RegionFile -> original vanilla 26.3 RegionFile reader parity.

A focused native Rust release-profile test uses SaveDir to write a command-block chunk into
an actual .mca region, drops the writer, reloads twice, changes styled output
and saves again, verifying one pending scheduled block tick. Then real Java
26.3 RegionFile, NbtIo and BlockEntity.loadStatic decode the *unchanged*
Horizon-produced file. This proves disk format interoperability for this
fixture, not a running vanilla server's chunk unload/reload lifecycle.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import uuid
try:
    from . import lab, test_rust_parity as base
except ImportError:
    import lab, test_rust_parity as base

ROOT=lab.ROOT
ORACLE=ROOT/'mods/horizon/src/test/java/dev/horizonlab/parity/VanillaCommandBlockRegionOracle.java'
HELPER=ORACLE.with_name('ParityData.java')
SOURCES=[ORACLE,HELPER,ROOT/'crates/horizon-client/src/save.rs',
         ROOT/'crates/horizon-client/src/command_block.rs',Path(__file__).resolve()]


def run(stage, command, log, extra_env=None):
    env=os.environ.copy()
    env.update(extra_env or {})
    with log.open('w') as output:
        subprocess.run(list(map(str,command)),cwd=ROOT,env=env,check=True,
                       stdout=output,stderr=subprocess.STDOUT,timeout=1200)


def main():
    basepath=lab.LOCAL/'command-block-region-parity'
    basepath.mkdir(parents=True,exist_ok=True)
    report=basepath/'report.json'
    report.unlink(missing_ok=True)
    lock,jars,libs=base.verify_artifacts()
    hashes=base.source_hashes(SOURCES)
    fixture=basepath/('fixture-'+uuid.uuid4().hex)
    classes=basepath/'classes'
    classes.mkdir(exist_ok=True)
    cp=os.pathsep.join(map(str,[jars['server'],*libs]))
    test='save::tests::command_block_region_disk_reopen_preserves_rich_output_and_pending_ticks'
    run('rust',['cargo','test','--locked','--release','-p','horizon-client','--lib',test,'--','--exact','--nocapture'],
        basepath/'rust.log',{'HORIZON_COMMAND_BLOCK_REGION_FIXTURE_DIR':str(fixture)})
    region=fixture/'dimensions/minecraft/overworld/region/r.-1.0.mca'
    if not region.is_file():raise AssertionError('Horizon produced no region file')
    raw=region.read_bytes(); region_hash=hashlib.sha256(raw).hexdigest()
    if len(raw)<8192 or len(raw)%4096:raise AssertionError('invalid region sector length')
    run('javac',['javac','--release',str(lock['minecraft']['java_major']),'-proc:none',
        '-cp',cp,'-d',classes,HELPER,ORACLE],basepath/'javac.log')
    run('vanilla',['java','-Xmx1G','-cp',os.pathsep.join([str(classes),cp]),
        'dev.horizonlab.parity.VanillaCommandBlockRegionOracle',
        fixture/'dimensions/minecraft/overworld'],basepath/'java.log')
    if 'VANILLA_REGION_FILE_PASS' not in (basepath/'java.log').read_text():
        raise AssertionError('vanilla reader did not confirm expected state')
    if hashlib.sha256(region.read_bytes()).hexdigest()!=region_hash:
        raise AssertionError('vanilla region reader unexpectedly modified the input')
    base.ensure_sources_unchanged(SOURCES,hashes)
    result={
        'schema':1,'minecraft':lock['minecraft']['version'],
        'manifest_sha1':lock['minecraft']['manifest_sha1'],
        'java':subprocess.check_output(['java','-version'],stderr=subprocess.STDOUT,text=True).splitlines()[0],
        'region_size':len(raw),'region_sha256':region_hash,'source_sha256':hashes,
        'assertions':['Horizon SaveDir writes physical .mca file and reopens it twice',
            'styled LastOutput, last execution, success count and powered state survive',
            'scheduled command block tick survives with relative delay and priority',
            'vanilla 26.3 RegionFile reads Horizon file without rewriting it',
            'vanilla NbtIo decodes chunk and BlockEntity.loadStatic decodes command block'],
        'scope':'Original vanilla Java RegionFile, NbtIo and BlockEntity loader interoperability for a Horizon-written chunk; no running ServerLevel/world simulation.'
    }
    lab.write_json(report,result)
    shutil.rmtree(fixture)
    print(f'command-block region interoperability passed: vanilla Java 26.3 read Horizon .mca ({len(raw)} bytes), verified command/tick/reload; report {report}',flush=True)

if __name__=='__main__':main()
