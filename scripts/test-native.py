#!/usr/bin/env python3
"""Build native fixtures, then execute the Rust integration harness sequentially."""
import argparse
import os
from pathlib import Path
import subprocess

ROOT=Path(__file__).resolve().parent.parent


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--skip-build',action='store_true')
    parser.add_argument('--include-debugger',action='store_true')
    parser.add_argument('--filter',default='')
    args=parser.parse_args()
    if not args.skip_build:
        commands=[
            ['cargo','build','--locked','-p','bed','--features','test-support','--bin','bed-native-fixture'],
            ['cargo','build','--locked','-p','bedtime-desktop','--examples'],
            ['cargo','build','--locked','-p','bedtime','--features','language-services','--examples'],
            ['cargo','build','--locked','-p','bedtime-example-terminal','--example','terminal_smoke'],
            ['cargo','build','--locked','-p','bedtime-example-layouts','-p','bedtime-example-editor','-p','bedtime-example-model','-p','bedtime-example-game-editor','-p','bedtime-example-game-runtime'],
        ]
        for command in commands:subprocess.run(command,cwd=ROOT,check=True)
    command=['cargo','test','--locked','-p','bed','-p','bedtime',
             '--features','bed/test-support,bedtime/test-support','--test','native',
             '--',args.filter,'--ignored','--test-threads=1','--nocapture']
    if not args.include_debugger:command+=['--skip','debugger_smoke']
    env=os.environ.copy()
    target=Path(env.get('CARGO_TARGET_DIR',ROOT/'target'))
    if not target.is_absolute():target=ROOT/target
    env['BED_NATIVE_FIXTURE_DIR']=str(target/'debug/examples')
    env['BED_NATIVE_APP_DIR']=str(target/'debug')
    subprocess.run(command,cwd=ROOT,env=env,check=True)


if __name__=='__main__':main()
