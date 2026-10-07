import os
import sys
from pathlib import Path
import pytest

# Ensure gulms is on sys.path
repo_root = Path(__file__).parent.parent
if str(repo_root) not in sys.path:
    sys.path.insert(0, str(repo_root))

@pytest.fixture
def fixtures_dir():
    return Path(__file__).parent / "fixtures"

@pytest.fixture
def benchmark_dir(fixtures_dir):
    env_dir = os.environ.get("BENCHMARK_DIR")
    if env_dir:
        p = Path(env_dir)
        if p.exists():
            return p
    # Default fallback to fixtures_dir
    return fixtures_dir
