import json
import sys
from pathlib import Path
from packaging.version import Version

if Path(__file__).with_name("prepared").read_text() != "ready":
    raise RuntimeError("project preparation did not complete")
value = json.load(sys.stdin)
version = Version(value["version"])
json.dump({"normalized": str(version), "prerelease": version.is_prerelease}, sys.stdout)
