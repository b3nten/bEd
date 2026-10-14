#!/usr/bin/env python3
"""Run the independent application's external automation and restart scenario."""
import argparse
from pathlib import Path
import sys
sys.path.insert(0,str(Path(__file__).resolve().parents[2]/"tools/automation"))
from run_smokes import bedside_scenario


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--binary",type=Path,default=Path("target/debug/bedside-table"))
    parser.add_argument("--capture-directory",type=Path,default=Path("target/bedside-captures"))
    args=parser.parse_args()
    args.capture_directory.mkdir(parents=True,exist_ok=True)
    bedside_scenario(args.binary,args.capture_directory)


if __name__=="__main__":main()
