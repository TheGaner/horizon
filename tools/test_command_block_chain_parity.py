#!/usr/bin/env python3
"""Actual Java 26.3 vs Horizon: conditional command chains and timed repeating blocks.

Run identical isolated vanilla-generated, force-loaded world copies on real
headless dedicated servers. An east-facing chain has successful conditional
links, a failed predicate, a skipped conditional successor, unconditional
recovery and a successful conditional successor. Separately the world is
frozen and a repeating command block is armed by live NBT; exactly five
server ticks must execute it five times, independent of wall-clock FPS.

Only this narrow runtime parity is asserted, not every command/packet shape.
"""
import json
from pathlib import Path
import re
import subprocess
import time
import uuid

try:
    from . import lab, test_rust_parity as base
    from .test_dedicated_server_parity import Server, free_port, write_server_dir
    from .test_command_block_horizon_server_parity import get_worlds, reply
except ImportError:
    import lab, test_rust_parity as base
    from test_dedicated_server_parity import Server, free_port, write_server_dir
    from test_command_block_horizon_server_parity import get_worlds, reply

ROOT = lab.ROOT
NAME = 'command-block-chain'
PARTS = [
    Path(__file__).resolve(),
    ROOT/'tools/test_command_block_horizon_server_parity.py',
    ROOT/'tools/test_dedicated_server_parity.py',
    ROOT/'crates/horizon-client/src/command_block.rs',
    ROOT/'crates/horizon-client/src/game/command_blocks.rs',
    ROOT/'crates/horizon-client/src/game/commands.rs',
    ROOT/'crates/horizon-client/src/game/commands/data.rs',
    ROOT/'crates/horizon-client/src/game.rs',
    ROOT/'crates/horizon-client/src/save.rs',
]
CHAIN_COMMANDS = {
    'a': 'scoreboard players add #a cbchain 1',
    'b': 'scoreboard players add #b cbchain 1',
    'c': 'scoreboard players add #c cbchain 1',
    'd': 'execute if score #a cbchain matches 99 run scoreboard players add #d cbchain 1',
    'e': 'scoreboard players add #e cbchain 1',
    'f': 'scoreboard players add #f cbchain 1',
    'g': 'scoreboard players add #g cbchain 1',
}


def main():
    lock, _, _ = base.verify_artifacts()
    assert lock['minecraft']['version'] == '26.3'
    source_world, _ = get_worlds()
    hashes = base.source_hashes(PARTS)
    output = lab.LOCAL/NAME
    output.mkdir(parents=True, exist_ok=True)
    report = output/'report.json'
    report.unlink(missing_ok=True)
    run = output/('run-'+uuid.uuid4().hex)
    run.mkdir()
    with (output/'build.log').open('w') as logfile:
        subprocess.run(['cargo', 'build', '--locked', '--release', '-p', 'horizon-client',
                        '--bin', 'horizon-client'], check=True, cwd=ROOT,
                       stdout=logfile, stderr=subprocess.STDOUT, timeout=900)
    meta = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=ROOT, text=True))
    binary = Path(meta['target_directory'])/'release/horizon-client'
    executables = {
        'vanilla': ['java', '-Xmx2g', '-jar', lab.LOCAL/'minecraft/26.3/jars/server.jar', '--nogui'],
        'horizon': [binary, '--resources', lab.LOCAL/'horizon-client/26.3', '--dedicated', '--nogui'],
    }
    expected = {'a': 1, 'b': 1, 'c': 1, 'd': None, 'e': None, 'f': 1, 'g': 1}
    outcomes = {}
    for engine, executable in executables.items():
        folder = run/engine
        write_server_dir(folder, free_port(), source_world)
        with (folder/'server.properties').open('a') as f:
            f.write('enable-command-block=true\npause-when-empty-seconds=-1\n')
        server = Server(executable, folder, engine)
        serial = 0

        def ask(command):
            nonlocal serial
            serial += 1
            response = '\n'.join(reply(server, command, serial))
            if 'Unknown command' in response or 'Unknown or incomplete command' in response:
                raise AssertionError(f'{engine}: unknown command: {command}: {response}')
            return response

        def score(objective, player, wanted):
            response = ask(f'scoreboard players get #{player} {objective}')
            if wanted is None:
                if 'none is set' not in response:
                    raise AssertionError(f'{engine}: #{player} unexpected score: {response}')
            elif f'#{player} has {wanted} [{objective}]' not in response:
                raise AssertionError(f'{engine}: #{player} expected {wanted}: {response}')
            return wanted

        def success_at(x, wanted):
            response = ask(f'data get block {x} 64 96 SuccessCount')
            if not re.search(r'block data: ' + str(wanted) + r'(?!\d)', response):
                raise AssertionError(f'{engine}: SuccessCount at {x} expected {wanted}: {response}')
            return wanted

        try:
            server.wait_for(r'Done \(', 180)
            ask('scoreboard objectives add cbchain dummy')
            for letter in 'bcdefg':
                x = 100 + ord(letter) - ord('a')
                conditional = 'true' if letter in 'bcdeg' else 'false'
                ask(f'setblock {x} 64 96 minecraft:chain_command_block[facing=east,conditional={conditional}]')
                ask(f'data merge block {x} 64 96 {{Command:"{CHAIN_COMMANDS[letter]}",auto:1b,TrackOutput:1b}}')
            ask('setblock 100 64 96 minecraft:command_block[facing=east]')
            ask(f'data merge block 100 64 96 {{Command:"{CHAIN_COMMANDS["a"]}",auto:1b,TrackOutput:1b}}')
            deadline = time.monotonic() + 25
            while time.monotonic() < deadline:
                if '#a has 1 [cbchain]' in ask('scoreboard players get #a cbchain'):
                    break
                time.sleep(.25)
            else:
                raise AssertionError(f'{engine}: initiating impulse did not execute')
            time.sleep(1)
            chain = {}
            for letter, desired in expected.items():
                x = 100 + ord(letter) - ord('a')
                chain[letter] = {'score': score('cbchain', letter, desired),
                                 'success_count': success_at(x, 0 if desired is None else 1)}
            print(engine, 'PASS 7-block chain: conditional success/failure, skip and unconditional recovery', flush=True)

            # Exercise the production maxCommandChainLength boundary in the
            # same world. The original impulse executes again, but a chain
            # limit of two allows only links b and c; f and g must remain at
            # their old scores rather than execute another time.
            ask('gamerule max_command_sequence_length 2')
            if not re.search(r'\b2\b', ask('gamerule max_command_sequence_length')):
                raise AssertionError(f'{engine}: chain length game rule was not set')
            ask('data merge block 100 64 96 {auto:0b}')
            ask('data merge block 100 64 96 {auto:1b}')
            deadline = time.monotonic() + 25
            while time.monotonic() < deadline:
                result = ask('scoreboard players get #a cbchain')
                if '#a has 2 [cbchain]' in result:
                    break
                time.sleep(.25)
            else:
                raise AssertionError(f'{engine}: second impulse failed: {result}')
            time.sleep(.5)
            limited = {'a': 2, 'b': 2, 'c': 2, 'd': None, 'e': None, 'f': 1, 'g': 1}
            for letter, desired in limited.items():
                score('cbchain', letter, desired)
            print(engine, 'PASS max_command_sequence_length=2 stops chain after two links', flush=True)

            # Each next chain block determines the direction of the next
            # traversal step, so this path bends south and then east.
            # Construct successors first so none can fire before the root.
            ask('gamerule max_command_sequence_length 65536')
            turns = [
                (121, 64, 96, 'south', 'false', 1),
                (121, 64, 97, 'south', 'true', 2),
                # This is a turn: the conditional block's *back* is west,
                # not the predecessor to its north. Vanilla would skip it
                # when conditional=true, so the bend itself is unconditional.
                (121, 64, 98, 'east', 'false', 3),
                (122, 64, 98, 'east', 'false', 4),
            ]
            for x, y, z, facing, is_conditional, link in turns:
                ask(f'setblock {x} {y} {z} minecraft:chain_command_block[facing={facing},conditional={is_conditional}]')
                ask(f'data merge block {x} {y} {z} {{Command:"scoreboard players add #turn{link} cbchain 1",auto:1b}}')
            ask('setblock 120 64 96 minecraft:command_block[facing=east]')
            ask('data merge block 120 64 96 {Command:"scoreboard players add #turn0 cbchain 1",auto:1b}')
            deadline = time.monotonic() + 25
            while time.monotonic() < deadline:
                if '#turn0 has 1 [cbchain]' in ask('scoreboard players get #turn0 cbchain'):
                    break
                time.sleep(.25)
            else:
                raise AssertionError(f'{engine}: turning chain root did not execute')
            time.sleep(.5)
            for link in range(5):
                score('cbchain', 'turn'+str(link), 1)
            print(engine, 'PASS east-south-east chain traverses turning links and conditional successors', flush=True)

            ask('scoreboard objectives add cbrepeat dummy')
            ask('setblock 110 64 96 minecraft:repeating_command_block[facing=east]')
            ask('data merge block 110 64 96 {Command:"scoreboard players add #ticks cbrepeat 1",auto:0b,TrackOutput:0b}')
            assert 'The game is frozen' in ask('tick freeze'), f'{engine}: could not freeze game ticks'
            ask('data merge block 110 64 96 {auto:1b}')
            score('cbrepeat', 'ticks', None)
            assert 'Stepping 5 tick(s)' in ask('tick step 5')
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                current = ask('scoreboard players get #ticks cbrepeat')
                if '#ticks has 5 [cbrepeat]' in current:
                    break
                if re.search(r'#ticks has ([6-9]|\d\d+) \[cbrepeat\]', current):
                    raise AssertionError(f'{engine}: repeating block ran too often: {current}')
                time.sleep(.25)
            else:
                raise AssertionError(f'{engine}: expected exactly five frozen stepped ticks, got {current}')
            success_at(110, 1)
            # A frozen world must not execute another repetition between
            # explicit tick steps, independent of host wall clock.
            time.sleep(.5)
            score('cbrepeat', 'ticks', 5)
            ask('data merge block 110 64 96 {auto:0b}')
            assert 'The game is running normally' in ask('tick unfreeze')
            outcomes[engine] = {'chain': chain, 'limited_chain_scores': limited, 'turning_chain': {f'turn{i}': 1 for i in range(5)}, 'repeat_step_count': 5, 'repeat_success_count': 1}
            print(engine, 'PASS repeating auto block executes exactly five explicit ticks', flush=True)
        finally:
            server.stop()
    assert outcomes['vanilla'] == outcomes['horizon']
    base.ensure_sources_unchanged(PARTS, hashes)
    lab.write_json(report, {
        'schema': 1, 'minecraft': '26.3', 'manifest_sha1': lock['minecraft']['manifest_sha1'],
        'saved_world_source': str(source_world), 'results': outcomes, 'source_hashes': hashes,
        'scope': 'Actual separate vanilla and Horizon dedicated servers, seven-block conditional chain, max-length=2 truncation, right-angle directional chain and exact five-step frozen repeating block; not arbitrary command or client packet parity',
    })
    print('PASS command-block-chain both Java26.3 and Horizon real servers match', flush=True)

if __name__ == '__main__':
    main()
