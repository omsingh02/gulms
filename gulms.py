#!/usr/bin/env python3
"""
GULMS CLI Runner
"""
import sys
from pathlib import Path

# Add package directory to sys.path
sys.path.insert(0, str(Path(__file__).parent))

from gulms.cli import main

if __name__ == "__main__":
    main()
